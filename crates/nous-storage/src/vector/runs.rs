use crate::{Database, WriteTransaction, codec::*, not_found, storage_error, write_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};
use std::collections::HashSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingClaim {
    pub source_hash: String,
    pub job_id: String,
}
impl Database {
    pub async fn embedding_space(&self, id: &str) -> Result<EmbeddingSpace> {
        space(&mut *self.pool.acquire().await.map_err(storage_error)?, id).await
    }
    pub async fn embedding_run(&self, id: &str) -> Result<EmbeddingRun> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let run = read(&mut tx, id).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(run)
    }
}
impl WriteTransaction {
    pub async fn embedding_space(&mut self, id: &str) -> Result<EmbeddingSpace> {
        space(&mut self.tx, id).await
    }
    pub async fn embedding_run(&mut self, id: &str) -> Result<EmbeddingRun> {
        read(&mut self.tx, id).await
    }
    pub async fn save_embedding_space(&mut self, value: &EmbeddingSpace) -> Result<()> {
        value.validate()?;
        sqlx::query("INSERT INTO embedding_spaces(id,provider_model,dimension,normalization,parameters) VALUES(?,?,?,?,?) ON CONFLICT(id) DO NOTHING")
            .bind(&value.id).bind(&value.provider_model).bind(value.dimension as i64).bind(enum_name(&value.normalization)?).bind(serde_json::to_string(&value.parameters)?).execute(&mut *self.tx).await.map_err(write_error)?;
        if self.embedding_space(&value.id).await? != *value {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding space identity is immutable",
            ));
        }
        Ok(())
    }
    pub async fn embedding_claims(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
    ) -> Result<Vec<EmbeddingClaim>> {
        sqlx::query("SELECT c.source_hash,c.job_id FROM embedding_content_claims c JOIN task_records j ON j.job_id=c.job_id WHERE c.namespace_id=? AND c.subject_id=? AND c.space_id=? AND j.state IN('queued','waiting','running') ORDER BY c.source_hash")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|row|Ok(EmbeddingClaim{source_hash:row.get("source_hash"),job_id:row.get("job_id")})).collect()
    }
    pub async fn claimed_embedding_hashes(&mut self, job: &str) -> Result<Vec<String>> {
        sqlx::query_scalar(
            "SELECT source_hash FROM embedding_content_claims WHERE job_id=? ORDER BY source_hash",
        )
        .bind(job)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(storage_error)
    }
    pub async fn release_embedding_claim(&mut self, job: &str, hash: &str) -> Result<()> {
        sqlx::query("DELETE FROM embedding_content_claims WHERE job_id=? AND source_hash=?")
            .bind(job)
            .bind(hash)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn insert_embedding_run(&mut self, value: &EmbeddingRun) -> Result<()> {
        let job = self.require_subject_job(&value.job).await?;
        let (namespace, subject) = job.subject_scope()?;
        self.embedding_space(&value.space_id).await?;
        if value.content_hashes.is_empty() {
            return Err(Error::invalid("embedding work needs content hashes"));
        }
        let claims = self
            .embedding_claims(namespace, subject, &value.space_id)
            .await?;
        let mut hashes = HashSet::new();
        for hash in &value.content_hashes {
            validate_embedding_hash(hash)?;
            if !hashes.insert(hash) {
                return Err(Error::invalid("embedding work repeats content"));
            }
            if let Some(claim) = claims.iter().find(|claim| claim.source_hash == *hash) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("embedding content already belongs to {}", claim.job_id),
                ));
            }
            if self
                .embedding_subscriptions(namespace, subject, &value.space_id, hash)
                .await?
                .is_empty()
            {
                return Err(Error::invalid(
                    "embedding content work has no accepted target subscription",
                ));
            }
        }
        sqlx::query("INSERT INTO embedding_runs(job_id,space_id,content_hashes) VALUES(?,?,?)")
            .bind(&job.id)
            .bind(&value.space_id)
            .bind(serde_json::to_string(&value.content_hashes)?)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        for hash in &value.content_hashes {
            sqlx::query("DELETE FROM embedding_content_claims WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? AND EXISTS(SELECT 1 FROM task_records j WHERE j.job_id=embedding_content_claims.job_id AND j.state NOT IN('queued','waiting','running'))")
                .bind(namespace.as_str()).bind(subject.as_str()).bind(&value.space_id).bind(hash).execute(&mut *self.tx).await.map_err(write_error)?;
            sqlx::query("INSERT INTO embedding_content_claims(namespace_id,subject_id,space_id,source_hash,job_id) VALUES(?,?,?,?,?)").bind(namespace.as_str()).bind(subject.as_str()).bind(&value.space_id).bind(hash).bind(&job.id).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        Ok(())
    }
}
pub(crate) async fn read(conn: &mut SqliteConnection, id: &str) -> Result<EmbeddingRun> {
    let row = sqlx::query("SELECT * FROM embedding_runs WHERE job_id=?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(EmbeddingRun {
        job: crate::authority::tasks::read(conn, id).await?,
        space_id: row.get("space_id"),
        content_hashes: serde_json::from_str(row.get("content_hashes"))?,
    })
}
pub(crate) async fn space(conn: &mut SqliteConnection, id: &str) -> Result<EmbeddingSpace> {
    let row = sqlx::query("SELECT * FROM embedding_spaces WHERE id=?")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .ok_or_else(not_found)?;
    Ok(EmbeddingSpace {
        id: row.get("id"),
        provider_model: row.get("provider_model"),
        dimension: row.get::<i64, _>("dimension") as usize,
        normalization: parse_enum(row.get("normalization"))?,
        parameters: serde_json::from_str(row.get("parameters"))?,
    })
}
