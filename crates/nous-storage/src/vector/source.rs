use crate::cognition::{memory::memory_row, structured::read::object};
use crate::{SourceStatus, not_found, storage_error};
use nous_domain::*;
use sqlx::SqliteConnection;
pub(crate) async fn validate_input(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    input: &EmbeddingInputItem,
) -> Result<()> {
    input.validate()?;
    if let Some(reference) = input.target.item().and_then(RetrievalTarget::cognition) {
        validate_reference(conn, namespace, subject, reference).await?;
        if input.text_content() != Some(text(conn, namespace, subject, reference).await?.as_str()) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding text differs from its authority revision",
            ));
        }
    }
    super::media_source::validate(conn, namespace, subject, input).await?;
    Ok(())
}

pub(crate) async fn text(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &CognitiveRef,
) -> Result<String> {
    if reference.kind == CognitiveKind::Memory {
        let memory = memory(
            conn,
            namespace,
            subject,
            &MemoryId(reference.id.clone()),
            reference.revision,
        )
        .await?
        .ok_or_else(not_found)?;
        return Ok(format!("{}\n{}", memory.title, memory.content));
    }
    let value = object(
        conn,
        namespace,
        &CognitiveId(reference.id.clone()),
        Some(reference.revision),
    )
    .await?;
    if value.subject_id != *subject || value.kind() != reference.kind {
        return Err(not_found());
    }
    let base = base_memory(conn, &value).await?;
    cognitive_embedding_text(&value, base.as_ref())
}
pub(crate) async fn memory(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    id: &MemoryId,
    revision: i64,
) -> Result<Option<Memory>> {
    sqlx::query("SELECT * FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?").bind(namespace.as_str()).bind(subject.as_str()).bind(id.as_str()).bind(revision).fetch_optional(conn).await.map_err(storage_error)?.map(memory_row).transpose()
}
pub(crate) async fn base_memory(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
) -> Result<Option<Memory>> {
    if let CognitiveContent::MemorySemantics(content) = &value.body {
        memory(
            conn,
            &value.namespace_id,
            &value.subject_id,
            &content.memory_id,
            content.memory_revision,
        )
        .await
    } else {
        Ok(None)
    }
}
pub(crate) async fn validate_reference(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &CognitiveRef,
) -> Result<()> {
    reference.validate()?;
    if reference.kind == CognitiveKind::Memory {
        let value = memory(
            conn,
            namespace,
            subject,
            &MemoryId(reference.id.clone()),
            reference.revision,
        )
        .await?
        .ok_or_else(not_found)?;
        if value.deleted {
            return Err(Error::new(
                ErrorCode::Conflict,
                "embedding revision is retired",
            ));
        }
        return Ok(());
    }
    let value = object(
        conn,
        namespace,
        &CognitiveId(reference.id.clone()),
        Some(reference.revision),
    )
    .await?;
    if value.subject_id != *subject || value.kind() != reference.kind {
        return Err(not_found());
    }
    if value.deleted {
        return Err(Error::new(
            ErrorCode::Conflict,
            "embedding source is retired",
        ));
    }
    let closure =
        crate::source::closure::closure(conn, namespace, subject, reference.clone()).await?;
    if closure
        .nodes
        .iter()
        .any(|node| node.status == SourceStatus::Missing)
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "embedding source closure has a missing reference",
        ));
    }
    Ok(())
}

impl crate::Database {
    pub async fn validate_embedding_input(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        let mut view = self.read_view().await?;
        validate_input(&mut view.tx, ns, subject, input).await?;
        view.finish().await
    }
}
impl crate::WriteTransaction {
    pub async fn current_embedding_input(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<EmbeddingInputItem> {
        reference.validate()?;
        let input = if reference.kind == CognitiveKind::Memory {
            let value = self.memory(ns, &MemoryId(reference.id.clone())).await?;
            if value.subject_id != *subject {
                return Err(not_found());
            }
            EmbeddingInputItem::for_memory(&value)?
        } else {
            let value = object(&mut self.tx, ns, &CognitiveId(reference.id.clone()), None).await?;
            if value.subject_id != *subject || value.kind() != reference.kind {
                return Err(not_found());
            }
            let base = base_memory(&mut self.tx, &value).await?;
            EmbeddingInputItem::for_cognition(&value, base.as_ref())?
        };
        validate_input(&mut self.tx, ns, subject, &input).await?;
        Ok(input)
    }
    pub async fn validate_embedding_input(
        &mut self,
        ns: &NamespaceId,
        subject: &SubjectId,
        input: &EmbeddingInputItem,
    ) -> Result<()> {
        validate_input(&mut self.tx, ns, subject, input).await
    }
}
