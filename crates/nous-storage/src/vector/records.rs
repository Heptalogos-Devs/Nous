use super::target_row;
use crate::{Database, ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
impl Database {
    pub async fn matching_embedding(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        matching(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            subject,
            space,
            hash,
        )
        .await
    }
    pub async fn embedding_record(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        target: &EmbeddingTarget,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        read(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            subject,
            space,
            target,
            hash,
        )
        .await
    }
    pub async fn embeddings_for_target(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<Vec<EmbeddingRecord>> {
        for_target(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            subject,
            target,
        )
        .await
    }
}
impl WriteTransaction {
    pub async fn matching_embedding(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        matching(&mut self.tx, namespace, subject, space, hash).await
    }
    pub async fn embedding_record(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        target: &EmbeddingTarget,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        read(&mut self.tx, namespace, subject, space, target, hash).await
    }
    pub async fn embeddings_for_target(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<Vec<EmbeddingRecord>> {
        for_target(&mut self.tx, namespace, subject, target).await
    }
    pub async fn save_embedding_record(
        &mut self,
        value: &EmbeddingRecord,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        let key = value.source_key()?;
        if input.target != value.target || input.source_hash() != value.source_hash {
            return Err(Error::invalid(
                "embedding record differs from its supplied actual input",
            ));
        }
        super::source::validate_input(&mut self.tx, &value.namespace_id, &value.subject_id, input)
            .await?;
        let space = self.embedding_space(&value.space_id).await?;
        if value.vector.len() != space.dimension
            || value.vector.iter().any(|v| !v.is_finite())
            || !value.vector.iter().any(|v| *v != 0.0)
        {
            return Err(Error::invalid(
                "embedding vector must be finite, nonzero and have the configured dimension",
            ));
        }
        let call = self.model_call(&value.model_call_id).await?;
        if call.state != ModelCallState::Succeeded
            || call.namespace_id != value.namespace_id
            || call.subject_id != value.subject_id
        {
            return Err(Error::invalid(
                "embedding requires a successful call in the same scope",
            ));
        }
        if let Some(existing) = read(
            &mut self.tx,
            &value.namespace_id,
            &value.subject_id,
            &value.space_id,
            &value.target,
            &value.source_hash,
        )
        .await?
        {
            if existing.source_hash != value.source_hash {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding identity already has different content",
                ));
            }
            return Ok(());
        }
        let identity = value.target.identity_key(&value.source_hash)?;
        let prior=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? AND model_call_id=? ORDER BY commit_seq,source_key LIMIT 1")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(&value.source_hash).bind(value.model_call_id.as_str()).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.map(row).transpose()?;
        let run = self.embedding_run(&call.job_id).await?;
        if run.space_id != space.id {
            return Err(Error::invalid("embedding call belongs to another space"));
        }
        if let Some(previous) = prior {
            if previous.vector != value.vector {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "content reuse must preserve accepted vector values",
                ));
            }
        } else {
            if matches!(run.job.state, JobState::Cancelled | JobState::Failed) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding task was retired",
                ));
            }
            validate_producer(&call, &space)?;
            let (index, _) = run
                .inputs
                .iter()
                .enumerate()
                .find(|(_, input)| {
                    input.source_hash() == value.source_hash && input.target == value.target
                })
                .ok_or_else(|| Error::invalid("embedding source was not submitted to this call"))?;
            let vectors: Vec<Vec<f32>> = serde_json::from_value(
                call.response
                    .as_ref()
                    .and_then(|value| value.get("result"))
                    .cloned()
                    .ok_or_else(|| Error::invalid("embedding call has no result"))?,
            )?;
            if vectors.len() != run.inputs.len() || vectors.get(index) != Some(&value.vector) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding values differ from the accepted call result",
                ));
            }
        }
        let bytes = value
            .vector
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO embedding_records(namespace_id,subject_id,space_id,target,source_key,identity_key,source_hash,dimension,vector,model_call_id,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(serde_json::to_string(&value.target)?).bind(key).bind(identity).bind(&value.source_hash).bind(space.dimension as i64).bind(bytes).bind(value.model_call_id.as_str()).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
impl ReadView {
    pub async fn matching_embedding(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        matching(&mut self.tx, namespace, subject, space, hash).await
    }
    pub async fn embedding_record(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        target: &EmbeddingTarget,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        read(&mut self.tx, namespace, subject, space, target, hash).await
    }
}
fn validate_producer(call: &ModelCall, space: &EmbeddingSpace) -> Result<()> {
    let producer: EmbeddingProducer = serde_json::from_value(
        call.request
            .get("embedding_producer")
            .cloned()
            .ok_or_else(|| Error::invalid("embedding call has no actual producer"))?,
    )?;
    let actual: EmbeddingSpace = serde_json::from_value(
        call.request
            .get("embedding_space")
            .cloned()
            .ok_or_else(|| Error::invalid("embedding call has no actual space"))?,
    )?;
    producer.validate()?;
    actual.validate()?;
    if actual != *space || producer.space_id != space.id || producer.resource_id != call.resource_id
    {
        return Err(Error::invalid(
            "actual embedding producer differs from its call or space",
        ));
    }
    Ok(())
}
pub(crate) async fn read(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space: &str,
    target: &EmbeddingTarget,
    hash: &str,
) -> Result<Option<EmbeddingRecord>> {
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_key=? AND source_hash=?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(target.source_key(hash)?).bind(hash).fetch_optional(conn).await.map_err(storage_error)?.map(row).transpose()
}
pub(crate) fn row(row: SqliteRow) -> Result<EmbeddingRecord> {
    let bytes: Vec<u8> = row.get("vector");
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::internal("invalid vector byte length"));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
        .collect();
    Ok(EmbeddingRecord {
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        space_id: row.get("space_id"),
        target: target_row(&row)?,
        source_hash: row.get("source_hash"),
        vector,
        model_call_id: ModelCallId(row.get("model_call_id")),
        commit_seq: row.get("commit_seq"),
    })
}
async fn for_target(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    target: &RetrievalTarget,
) -> Result<Vec<EmbeddingRecord>> {
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND identity_key=? ORDER BY space_id,commit_seq")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(target.identity_key()?).fetch_all(conn).await.map_err(storage_error)?.into_iter().map(row).collect()
}

async fn matching(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space: &str,
    hash: &str,
) -> Result<Option<EmbeddingRecord>> {
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=? ORDER BY commit_seq,source_key LIMIT 1")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(hash).fetch_optional(conn).await.map_err(storage_error)?.map(row).transpose()
}
