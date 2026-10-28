//! Exact named-source membership and atomic record retirement.
mod graph;
mod operations;
pub use operations::SourceOperationsPage;
mod preview;
mod restore;
mod trash_operations;
use crate::App;
use nous_domain::*;
use nous_storage::{BeginOperation, OperationReceipt, WriteTransaction, operation_key};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
pub use trash_operations::SourceTrashOperationsPage;

impl App {
    pub async fn source_directory(
        &self,
        ctx: &CallContext,
        request: &SourceDirectoryRequest,
    ) -> Result<SourceDirectory> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let mut view = self.database.read_view().await?;
        view.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let now = nous_foundation::clock::Clock::operation_snapshot().unix_ms();
        let rows = view
            .source_memory_entries(&request.namespace_id, &request.subject_id, now)
            .await?;
        let mut groups = BTreeMap::<
            String,
            (
                NamedSourceSummary,
                BTreeSet<MaterialId>,
                BTreeSet<ObservationId>,
            ),
        >::new();
        for row in rows {
            let Some(source) = row.source else {
                continue;
            };
            let reference = CognitiveRef::memory(&row.id, row.revision);
            match view
                .require_source_read(
                    ctx,
                    &request.namespace_id,
                    &request.subject_id,
                    &RetrievalTarget::Cognition(reference),
                )
                .await
            {
                Ok(()) => {}
                Err(error) if error.code == ErrorCode::Forbidden => continue,
                Err(error) => return Err(error),
            }
            let (summary, materials, observations) =
                groups.entry(source.clone()).or_insert_with(|| {
                    (
                        NamedSourceSummary {
                            source,
                            memory_count: 0,
                            material_count: 0,
                            observation_count: 0,
                            protected_count: 0,
                            last_updated: row.recorded_at.clone(),
                        },
                        BTreeSet::new(),
                        BTreeSet::new(),
                    )
                });
            summary.memory_count += 1;
            summary.protected_count += usize::from(row.protected);
            summary.last_updated = summary.last_updated.clone().max(row.recorded_at);
            materials.insert(row.material_id);
            observations.insert(row.observation_id);
            summary.material_count = materials.len();
            summary.observation_count = observations.len();
        }
        let result = SourceDirectory {
            namespace_id: request.namespace_id.clone(),
            subject_id: request.subject_id.clone(),
            watermark: view.watermark,
            items: groups
                .into_values()
                .map(|(summary, _, _)| summary)
                .collect(),
        };
        view.finish().await?;
        Ok(result)
    }
    pub async fn trash_sources(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        mut request: TrashSourcesRequest,
    ) -> Result<OperationReceipt> {
        request.validate()?;
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        request.sources.sort();
        request.sources.dedup();
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(ctx, "source.trash", &scope, operation_id, &request)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let result = self.trash_sources_in(ctx, &mut tx, request).await?;
        let receipt = tx.commit(json!(result)).await?;
        self.tasks.notify_wake();
        Ok(receipt)
    }
    async fn trash_sources_in(
        &self,
        ctx: &CallContext,
        tx: &mut WriteTransaction,
        request: TrashSourcesRequest,
    ) -> Result<TrashSourcesResult> {
        tx.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let rows = tx
            .source_memory_entries(&request.namespace_id, &request.subject_id)
            .await?;
        let selected: Vec<_> = rows
            .into_iter()
            .filter(|row| {
                row.source
                    .as_ref()
                    .is_some_and(|source| request.sources.contains(source))
            })
            .collect();
        // Authorize the complete exact group before changing even one record.
        for row in &selected {
            tx.require_source_read(
                ctx,
                &request.namespace_id,
                &request.subject_id,
                &RetrievalTarget::Cognition(CognitiveRef::memory(&row.id, row.revision)),
            )
            .await?;
        }
        if !request.force && selected.iter().any(|row| row.protected) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "source selection contains protected memories; explicitly force or unprotect them",
            ));
        }
        let mut affected = BTreeSet::new();
        for row in &selected {
            for target in tx
                .memory_source_dependents(&request.namespace_id, &row.id)
                .await?
            {
                if let RetrievalTarget::Cognition(reference) = target
                    && !matches!(
                        reference.kind,
                        CognitiveKind::Memory
                            | CognitiveKind::MemorySemantics
                            | CognitiveKind::Entity
                            | CognitiveKind::Association
                    )
                {
                    affected.insert(reference);
                }
            }
        }
        let graph = self
            .retire_source_graph_in(tx, &request.namespace_id, &request.subject_id, &selected)
            .await?;
        let mut trashed = Vec::with_capacity(selected.len());
        let mut retired_vector_associations = graph.retired_vector_associations;
        for row in &selected {
            let value = self
                .change_memory_in(
                    tx,
                    &request.namespace_id,
                    &row.id,
                    &MemoryChange {
                        action: MemoryAction::Trash,
                        expected_revision: row.revision,
                        strength: None,
                        hours: None,
                        reason: if request.reason.is_empty() {
                            "source_trash".into()
                        } else {
                            request.reason.clone()
                        },
                        force: request.force,
                        edit: None,
                    },
                )
                .await?;
            retired_vector_associations += value.retired_vector_associations;
            let memory: Memory = serde_json::from_value(value.result)?;
            trashed.push(CognitiveRef::memory(&memory.id, memory.revision));
        }
        let matched_sources = selected
            .iter()
            .filter_map(|row| row.source.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Ok(TrashSourcesResult {
            sources: request.sources,
            matched_sources,
            trashed_memories: trashed,
            graph,
            affected_independent: affected.into_iter().collect(),
            force: request.force,
            retired_vector_associations,
        })
    }
}
