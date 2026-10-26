use super::target_row;
use crate::{Database, ReadView, WriteTransaction, storage_error, write_error};
use nous_domain::*;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection, sqlite::SqliteRow};
const RECORD_SELECT: &str = "SELECT r.*,c.vector,c.model_call_id FROM embedding_records r JOIN embedding_contents c USING(namespace_id,subject_id,space_id,source_hash)";
macro_rules! reads {
    ($owner:ty) => {
        impl $owner {
            pub async fn matching_embedding(
                &mut self,
                namespace: &NamespaceId,
                subject: &SubjectId,
                space: &str,
                hash: &str,
            ) -> Result<Option<EmbeddingContent>> {
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
    };
}
reads!(ReadView);
reads!(WriteTransaction);
impl Database {
    pub async fn matching_embedding(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Option<EmbeddingContent>> {
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
    pub async fn embeddings_for_target(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        target: &RetrievalTarget,
    ) -> Result<Vec<EmbeddingRecord>> {
        for_target(&mut self.tx, namespace, subject, target).await
    }
    pub async fn save_embedding_content(
        &mut self,
        value: &EmbeddingContent,
    ) -> Result<EmbeddingContent> {
        self.check_sequence(value.commit_seq)?;
        validate_embedding_hash(&value.source_hash)?;
        let space = self.embedding_space(&value.space_id).await?;
        if value.vector.len() != space.dimension
            || value.vector.iter().any(|value| !value.is_finite())
            || !value.vector.iter().any(|value| *value != 0.0)
        {
            return Err(Error::invalid(
                "embedding content vector must be finite, nonzero and match the space dimension",
            ));
        }
        let call = self.model_call(&value.model_call_id).await?;
        if call.state != ModelCallState::Succeeded
            || call.namespace_id != value.namespace_id
            || call.subject_id != value.subject_id
        {
            return Err(Error::invalid(
                "embedding content requires its actual successful scoped call",
            ));
        }
        let run = self.embedding_run(&call.job_id).await?;
        if run.space_id != space.id || !run.content_hashes.contains(&value.source_hash) {
            return Err(Error::invalid(
                "embedding call does not own this content space",
            ));
        }
        validate_producer(&call, &space)?;
        if !super::embedding_call_contents(&call)?
            .iter()
            .any(|content| {
                content.source_hash == value.source_hash
                    && content.vector.as_ref() == Some(&value.vector)
            })
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding values differ from the actual call result",
            ));
        }
        if let Some(existing) = self
            .matching_embedding(
                &value.namespace_id,
                &value.subject_id,
                &value.space_id,
                &value.source_hash,
            )
            .await?
        {
            return Ok(existing);
        }
        let bytes = value
            .vector
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        sqlx::query("INSERT INTO embedding_contents(namespace_id,subject_id,space_id,source_hash,dimension,vector,model_call_id,commit_seq) VALUES(?,?,?,?,?,?,?,?)")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(&value.source_hash).bind(space.dimension as i64).bind(bytes).bind(value.model_call_id.as_str()).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(value.clone())
    }
    pub async fn save_embedding_record(
        &mut self,
        value: &EmbeddingRecord,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        if input.target != value.target || input.source_hash() != value.source_hash {
            return Err(Error::invalid(
                "embedding target record differs from its actual subscribed parts",
            ));
        }
        self.validate_embedding_input(&value.namespace_id, &value.subject_id, input)
            .await?;
        let content = self
            .matching_embedding(
                &value.namespace_id,
                &value.subject_id,
                &value.space_id,
                &value.source_hash,
            )
            .await?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Conflict,
                    "embedding content has not been accepted",
                )
            })?;
        if content.model_call_id != value.model_call_id || content.vector != value.vector {
            return Err(Error::new(
                ErrorCode::Conflict,
                "content reuse must preserve the actual call and accepted vector values",
            ));
        }
        sqlx::query("INSERT INTO embedding_records(namespace_id,subject_id,space_id,source_hash,target,source_key,identity_key,commit_seq) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT DO NOTHING")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(&value.space_id).bind(&value.source_hash).bind(serde_json::to_string(&value.target)?).bind(value.source_key()?).bind(value.target.identity_key(&value.source_hash)?).bind(value.commit_seq).execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
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
    let mut query = QueryBuilder::<Sqlite>::new(RECORD_SELECT);
    query
        .push(" WHERE r.namespace_id=")
        .push_bind(namespace.as_str())
        .push(" AND r.subject_id=")
        .push_bind(subject.as_str())
        .push(" AND r.space_id=")
        .push_bind(space)
        .push(" AND r.source_key=")
        .push_bind(target.source_key(hash)?)
        .push(" AND r.source_hash=")
        .push_bind(hash);
    query
        .build()
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(row)
        .transpose()
}
pub(crate) fn row(row: SqliteRow) -> Result<EmbeddingRecord> {
    Ok(EmbeddingRecord {
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        space_id: row.get("space_id"),
        target: target_row(&row)?,
        source_hash: row.get("source_hash"),
        vector: vector(&row)?,
        model_call_id: ModelCallId(row.get("model_call_id")),
        commit_seq: row.get("commit_seq"),
    })
}
fn vector(row: &SqliteRow) -> Result<Vec<f32>> {
    let bytes: Vec<u8> = row.get("vector");
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::internal("invalid vector byte length"));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
        .collect())
}
async fn for_target(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    target: &RetrievalTarget,
) -> Result<Vec<EmbeddingRecord>> {
    let mut query = QueryBuilder::<Sqlite>::new(RECORD_SELECT);
    query
        .push(" WHERE r.namespace_id=")
        .push_bind(namespace.as_str())
        .push(" AND r.subject_id=")
        .push_bind(subject.as_str())
        .push(" AND r.identity_key=")
        .push_bind(target.identity_key()?)
        .push(" ORDER BY r.space_id,r.commit_seq");
    query
        .build()
        .fetch_all(conn)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(row)
        .collect()
}
async fn matching(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space: &str,
    hash: &str,
) -> Result<Option<EmbeddingContent>> {
    sqlx::query("SELECT * FROM embedding_contents WHERE namespace_id=? AND subject_id=? AND space_id=? AND source_hash=?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(space).bind(hash).fetch_optional(conn).await.map_err(storage_error)?.map(|row|Ok(EmbeddingContent{namespace_id:NamespaceId(row.get("namespace_id")),subject_id:SubjectId(row.get("subject_id")),space_id:row.get("space_id"),source_hash:row.get("source_hash"),vector:vector(&row)?,model_call_id:ModelCallId(row.get("model_call_id")),commit_seq:row.get("commit_seq")})).transpose()
}
