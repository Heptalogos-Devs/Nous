use crate::App;
use nous_domain::*;
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde_json::json;

impl App {
    pub async fn restore_sources(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: RestoreSourcesRequest,
    ) -> Result<OperationReceipt> {
        request.validate()?;
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let key = operation_key(
            ctx,
            "source.restore",
            &format!(
                "namespace:{}/subject:{}",
                request.namespace_id, request.subject_id
            ),
            operation_id,
            &request,
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let original = tx
            .source_trash_receipt(
                &request.namespace_id,
                &request.subject_id,
                request.trash_commit_seq,
            )
            .await?;
        let previous: TrashSourcesResult = serde_json::from_value(original.result)?;
        // Validate the whole undo set before writing any replacement revision.
        for reference in &previous.trashed_memories {
            let memory = tx
                .memory(&request.namespace_id, &MemoryId(reference.id.clone()))
                .await
                .map_err(|error| {
                    if error.code == ErrorCode::NotFound {
                        Error::new(
                            ErrorCode::Conflict,
                            "source undo includes an erased or unavailable Memory",
                        )
                    } else {
                        error
                    }
                })?;
            if reference.kind != CognitiveKind::Memory
                || memory.subject_id != request.subject_id
                || memory.revision != reference.revision
                || !memory.deleted
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "source undo would replace a later Memory change",
                ));
            }
            tx.require_source_read(
                ctx,
                &request.namespace_id,
                &request.subject_id,
                &RetrievalTarget::Cognition(reference.clone()),
            )
            .await?;
        }
        let mut graph = Vec::new();
        for (references, deleted) in [
            (&previous.graph.revised_entities, false),
            (&previous.graph.retired_entities, true),
            (&previous.graph.revised_associations, false),
            (&previous.graph.retired_associations, true),
            (&previous.graph.retired_annotations, true),
        ] {
            for reference in references {
                let current = tx
                    .structured_object(&request.namespace_id, &CognitiveId(reference.id.clone()))
                    .await?;
                if current.subject_id != request.subject_id
                    || current.reference() != *reference
                    || current.deleted != deleted
                {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "source undo would replace a later graph change",
                    ));
                }
                let revision = reference
                    .revision
                    .checked_sub(1)
                    .filter(|revision| *revision > 0)
                    .ok_or_else(|| Error::invalid("source graph undo has no prior revision"))?;
                let before = tx
                    .structured_revision(&request.namespace_id, &current.id, revision)
                    .await?;
                graph.push((current, before));
            }
        }
        let mut result = RestoreSourcesResult {
            trash_commit_seq: request.trash_commit_seq,
            restored_memories: vec![],
            graph: SourceGraphChanges::default(),
            vector_associations_reused: 0,
        };
        let reason = if request.reason.is_empty() {
            "source_restore"
        } else {
            request.reason.as_str()
        };
        for reference in previous.trashed_memories {
            let memory = self
                .restore_memory_snapshot_in(
                    &mut tx,
                    &request.namespace_id,
                    &MemoryId(reference.id),
                    reference.revision,
                    reason,
                )
                .await?;
            result
                .restored_memories
                .push(CognitiveRef::memory(&memory.id, memory.revision));
        }
        // Restore annotations and entities before associations that reference them.
        graph.sort_by_key(|(_, before)| match before.kind() {
            CognitiveKind::MemorySemantics => 0,
            CognitiveKind::Entity => 1,
            _ => 2,
        });
        for (mut current, before) in graph {
            current.body = before.body;
            current.evidence = before.evidence;
            current.times = before.times;
            current.deleted = before.deleted;
            let (reference, retired) = self.save_source_graph_revision(&mut tx, current).await?;
            result.graph.retired_vector_associations += retired;
            match reference.kind {
                CognitiveKind::MemorySemantics => result.graph.restored_annotations.push(reference),
                CognitiveKind::Entity
                    if previous
                        .graph
                        .retired_entities
                        .iter()
                        .any(|old| old.id == reference.id) =>
                {
                    result.graph.restored_entities.push(reference)
                }
                CognitiveKind::Entity => result.graph.revised_entities.push(reference),
                CognitiveKind::Association
                    if previous
                        .graph
                        .retired_associations
                        .iter()
                        .any(|old| old.id == reference.id) =>
                {
                    result.graph.restored_associations.push(reference)
                }
                CognitiveKind::Association => result.graph.revised_associations.push(reference),
                _ => {
                    return Err(Error::internal(
                        "source undo encountered an unrelated cognitive kind",
                    ));
                }
            }
        }
        for reference in result
            .restored_memories
            .iter()
            .chain(&result.graph.restored_annotations)
            .chain(&result.graph.revised_entities)
            .chain(&result.graph.restored_entities)
            .chain(&result.graph.restored_associations)
            .chain(&result.graph.revised_associations)
        {
            result.vector_associations_reused += tx
                .reuse_retained_cognitive_vectors(
                    &request.namespace_id,
                    &request.subject_id,
                    reference,
                )
                .await?;
        }
        let receipt = tx.commit(json!(result)).await?;
        self.tasks.notify_wake();
        Ok(receipt)
    }
}
