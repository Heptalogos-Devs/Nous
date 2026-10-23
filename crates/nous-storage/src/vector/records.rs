use super::{target_kind, target_row};
use crate::{Database, WriteTransaction, codec::*, storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
impl Database {
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
    pub async fn embeddings_for_cognition(
        &self,
        namespace: &NamespaceId,
        reference: &CognitiveRef,
    ) -> Result<Vec<EmbeddingRecord>> {
        for_cognition(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            reference,
        )
        .await
    }
}
impl WriteTransaction {
    pub async fn embeddings_for_cognition(
        &mut self,
        namespace: &NamespaceId,
        reference: &CognitiveRef,
    ) -> Result<Vec<EmbeddingRecord>> {
        for_cognition(&mut self.tx, namespace, reference).await
    }
    pub async fn embedding_record(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        target: &EmbeddingTarget,
        hash: &str,
    ) -> Result<Option<EmbeddingRecord>> {
        read(&mut self.tx, ns, subject, space, target, hash).await
    }

    pub async fn save_embedding_record(&mut self, value: &EmbeddingRecord) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        value.source_key()?;
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
            || call.resource_id != space.resource_id
        {
            return Err(Error::invalid(
                "embedding requires a successful call in the same scope and resource",
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
                    "embedding source identity already has different content",
                ));
            }
            return Ok(());
        }
        let run = self.embedding_run(&call.job_id).await?;
        if run.space_id != space.id || run.producer_signature != space.producer_signature {
            return Err(Error::invalid(
                "embedding call belongs to a different space",
            ));
        }
        let prior = if let Some(reference) = value.reference() {
            sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND cognition_kind=? AND cognition_id=? AND source_hash=? AND model_call_id=? LIMIT 1").bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(enum_name(&reference.kind)?).bind(&reference.id).bind(&value.source_hash).bind(value.model_call_id.as_str()).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.map(row).transpose()?
        } else {
            None
        };
        let text = if let Some(previous) = prior {
            if previous.vector != value.vector {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "content reuse must preserve accepted vector values",
                ));
            }
            super::source::text(
                &mut self.tx,
                &value.namespace_id,
                &value.subject_id,
                value.reference().expect("cognition reuse"),
            )
            .await?
        } else {
            if matches!(run.job.state, JobState::Cancelled | JobState::Failed) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding task was retired",
                ));
            }
            let (index, input) = run
                .inputs
                .iter()
                .enumerate()
                .find(|(_, input)| {
                    input.source_hash() == value.source_hash
                        && same_identity(&input.target, &value.target)
                })
                .ok_or_else(|| Error::invalid("embedding source was not submitted to this call"))?;
            let vectors: Vec<Vec<f32>> = serde_json::from_value(
                call.response
                    .as_ref()
                    .and_then(|v| v.get("result"))
                    .cloned()
                    .ok_or_else(|| Error::invalid("embedding call has no result"))?,
            )
            .map_err(|_| Error::invalid("embedding call result is not a vector array"))?;
            if vectors.len() != run.inputs.len() || vectors.get(index) != Some(&value.vector) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "embedding values differ from the accepted call result",
                ));
            }
            input.text.clone()
        };
        let input = EmbeddingInputItem {
            target: value.target.clone(),
            text,
        };
        if input.source_hash() != value.source_hash {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding hash differs from source text",
            ));
        }
        super::source::validate_input(&mut self.tx, &value.namespace_id, &value.subject_id, &input)
            .await?;
        let reference = value.reference();
        let bytes = value
            .vector
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO embedding_records(namespace_id,subject_id,space_id,target_kind,cognition_kind,cognition_id,cognition_revision,source_hash,dimension,vector,model_call_id,commit_seq) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)")
   .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(target_kind(&value.target)).bind(reference.map(|r|enum_name(&r.kind)).transpose()?).bind(reference.map(|r|r.id.as_str())).bind(reference.map(|r|r.revision)).bind(&value.source_hash).bind(space.dimension as i64).bind(bytes).bind(value.model_call_id.as_str()).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }
}
fn same_identity(a: &EmbeddingTarget, b: &EmbeddingTarget) -> bool {
    match (a, b) {
        (EmbeddingTarget::Cognition(a), EmbeddingTarget::Cognition(b)) => {
            a.kind == b.kind && a.id == b.id
        }
        _ => a == b,
    }
}
pub(crate) async fn read(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space: &str,
    target: &EmbeddingTarget,
    hash: &str,
) -> Result<Option<EmbeddingRecord>> {
    let r = target.reference();
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND target_kind=? AND cognition_kind IS ? AND cognition_id IS ? AND cognition_revision IS ? AND (?='cognition' OR source_hash=?)").bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(target_kind(target)).bind(r.map(|r|enum_name(&r.kind)).transpose()?).bind(r.map(|r|r.id.as_str())).bind(r.map(|r|r.revision)).bind(target_kind(target)).bind(hash).fetch_optional(conn).await.map_err(storage_error)?.map(row).transpose()
}
pub(crate) fn row(row: SqliteRow) -> Result<EmbeddingRecord> {
    let bytes: Vec<u8> = row.get("vector");
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::internal("invalid vector byte length"));
    }
    let vector = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().expect("four bytes")))
        .collect::<Vec<_>>();
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

async fn for_cognition(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    reference: &CognitiveRef,
) -> Result<Vec<EmbeddingRecord>> {
    reference.validate()?;
    sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND cognition_kind=? AND cognition_id=? ORDER BY space_id,cognition_revision")
        .bind(namespace.as_str()).bind(enum_name(&reference.kind)?).bind(&reference.id)
        .fetch_all(conn).await.map_err(storage_error)?.into_iter().map(row).collect()
}
