use super::{target_kind, target_row};
use crate::{Database, WriteTransaction, codec::*, not_found, storage_error, write_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};
use std::collections::HashSet;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingClaim {
    pub target: EmbeddingTarget,
    pub source_hash: String,
    pub job_id: String,
}
impl EmbeddingClaim {
    pub fn source_key(&self) -> Result<String> {
        self.target.source_key(&self.source_hash)
    }
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
        if value.modality != "text" || value.dimension == 0 || value.dimension > 65536 {
            return Err(Error::invalid("invalid text embedding space"));
        }
        for name in [
            &value.id,
            &value.resource_id,
            &value.provider_model,
            &value.producer_signature,
        ] {
            validate_name(name, "embedding space field")?;
        }
        sqlx::query("INSERT INTO embedding_spaces(id,resource_id,provider_model,dimension,producer_signature,modality) VALUES(?,?,?,?,?,?) ON CONFLICT(id) DO NOTHING").bind(&value.id).bind(&value.resource_id).bind(&value.provider_model).bind(value.dimension as i64).bind(&value.producer_signature).bind(&value.modality).execute(&mut *self.tx).await.map_err(write_error)?;
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
        let rows=sqlx::query("SELECT i.* FROM embedding_inputs i JOIN embedding_runs r ON r.job_id=i.job_id JOIN task_records t ON t.job_id=r.job_id WHERE t.namespace_id=? AND t.subject_id=? AND r.space_id=? AND t.state IN('queued','waiting','running') ORDER BY i.job_id,i.ordinal").bind(namespace.as_str()).bind(subject.as_str()).bind(space).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        rows.into_iter()
            .map(|row| {
                let input = EmbeddingInputItem {
                    target: target_row(&row)?,
                    text: row.get("text"),
                };
                Ok(EmbeddingClaim {
                    target: input.target.clone(),
                    source_hash: input.source_hash(),
                    job_id: row.get("job_id"),
                })
            })
            .collect()
    }
    pub async fn insert_embedding_run(&mut self, value: &EmbeddingRun) -> Result<()> {
        let job = self.require_subject_job(&value.job).await?;
        let (namespace, subject) = job.subject_scope()?;
        let space = self.embedding_space(&value.space_id).await?;
        if space.producer_signature != value.producer_signature
            || value.inputs.is_empty()
            || value.inputs.len() > 256
        {
            return Err(Error::invalid(
                "embedding input batch or producer is invalid",
            ));
        }
        let claims = self
            .embedding_claims(namespace, subject, &value.space_id)
            .await?;
        let mut identities = HashSet::new();
        for input in &value.inputs {
            input.validate()?;
            let key = input.source_key()?;
            if !identities.insert(key.clone()) {
                return Err(Error::invalid("embedding batch repeats a source"));
            }
            if let Some(claim) = claims
                .iter()
                .find(|claim| claim.source_key().ok().as_ref() == Some(&key))
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    format!("embedding source already belongs to {}", claim.job_id),
                ));
            }
            super::source::validate_input(&mut self.tx, namespace, subject, input).await?;
        }
        sqlx::query("INSERT INTO embedding_runs(job_id,producer_signature,space_id) VALUES(?,?,?)")
            .bind(&job.id)
            .bind(&value.producer_signature)
            .bind(&value.space_id)
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        for (ordinal, input) in value.inputs.iter().enumerate() {
            let reference = input.target.reference();
            sqlx::query("INSERT INTO embedding_inputs(job_id,ordinal,target_kind,cognition_kind,cognition_id,cognition_revision,text) VALUES(?,?,?,?,?,?,?)").bind(&job.id).bind(ordinal as i64).bind(target_kind(&input.target)).bind(reference.map(|r|enum_name(&r.kind)).transpose()?).bind(reference.map(|r|r.id.as_str())).bind(reference.map(|r|r.revision)).bind(&input.text).execute(&mut *self.tx).await.map_err(write_error)?;
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
    let inputs = sqlx::query("SELECT * FROM embedding_inputs WHERE job_id=? ORDER BY ordinal")
        .bind(id)
        .fetch_all(&mut *conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(|row| {
            Ok(EmbeddingInputItem {
                target: target_row(&row)?,
                text: row.get("text"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(EmbeddingRun {
        job: crate::authority::tasks::read(conn, id).await?,
        producer_signature: row.get("producer_signature"),
        space_id: row.get("space_id"),
        inputs,
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
        resource_id: row.get("resource_id"),
        provider_model: row.get("provider_model"),
        dimension: row.get::<i64, _>("dimension") as usize,
        producer_signature: row.get("producer_signature"),
        modality: row.get("modality"),
    })
}
