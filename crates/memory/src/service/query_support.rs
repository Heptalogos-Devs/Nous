// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) async fn resolve_memory_references(
    service: &MemoryService,
    subject: SubjectId,
    references: &[CognitiveRef],
) -> Result<HashMap<CognitiveRef, (MemoryId, MemoryRevisionId)>> {
    let memory_ids = references
        .iter()
        .filter_map(|reference| match reference {
            CognitiveRef::Memory(value) => Some(value.0),
            _ => None,
        })
        .collect::<Vec<_>>();
    let revision_ids = references
        .iter()
        .filter_map(|reference| match reference {
            CognitiveRef::MemoryRevision(value) => Some(value.0),
            _ => None,
        })
        .collect::<Vec<_>>();
    if memory_ids.is_empty() && revision_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query("SELECT memory_id,current_revision_id,memory_revision_id,source_kind,source_id FROM (SELECT memory_id,current_revision_id AS current_revision_id,current_revision_id AS memory_revision_id,'memory'::text AS source_kind,memory_id AS source_id FROM memory_objects WHERE subject_id=$1 AND memory_id=ANY($2::uuid[]) UNION ALL SELECT memory_id,memory_revision_id,memory_revision_id,'memory_revision'::text,memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_revision_id=ANY($3::uuid[])) resolved")
        .bind(subject.0)
        .bind(&memory_ids)
        .bind(&revision_ids)
        .fetch_all(service.store.pool())
        .await
        .map_err(nous_persistence::database_error)?;
    let mut result = HashMap::new();
    for row in rows {
        let memory = MemoryId(
            row.try_get("memory_id")
                .map_err(nous_persistence::database_error)?,
        );
        let revision = MemoryRevisionId(
            row.try_get("memory_revision_id")
                .map_err(nous_persistence::database_error)?,
        );
        let source_kind: String = row
            .try_get("source_kind")
            .map_err(nous_persistence::database_error)?;
        let source_id: Uuid = row
            .try_get("source_id")
            .map_err(nous_persistence::database_error)?;
        let reference = if source_kind == "memory" {
            CognitiveRef::Memory(MemoryId(source_id))
        } else {
            CognitiveRef::MemoryRevision(MemoryRevisionId(source_id))
        };
        result.insert(reference, (memory, revision));
    }
    Ok(result)
}

pub(super) async fn resolve_memory_references_in_view(
    service: &MemoryService,
    subject: SubjectId,
    references: &[CognitiveRef],
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<HashMap<CognitiveRef, (MemoryId, MemoryRevisionId)>> {
    let Some(view) = view else {
        return resolve_memory_references(service, subject, references).await;
    };
    let mut resolved = HashMap::new();
    for requested in references {
        let Some(state) = view.cognition_for(requested) else {
            continue;
        };
        let CognitiveRef::Memory(memory) = state.object else {
            continue;
        };
        let reference = if &state.object == requested {
            &state.head
        } else {
            requested
        };
        if let CognitiveRef::MemoryRevision(revision) = reference {
            resolved.insert(requested.clone(), (memory, *revision));
        }
    }
    Ok(resolved)
}

pub(super) async fn canonical_preferences(
    service: &MemoryService,
    subject: SubjectId,
    references: Vec<CognitiveRef>,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<Vec<CognitiveRef>> {
    let mut result = Vec::new();
    for reference in references {
        let CognitiveRef::Tag(tag) = reference else {
            result.push(reference);
            continue;
        };
        let canonical = if let Some(view) = view {
            view.canonical_tag(tag).ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "historical Tag unavailable",
                )
            })
        } else {
            service.store.canonical_tag_id(subject, tag).await
        };
        match canonical {
            Ok(tag) => result.push(CognitiveRef::Tag(tag)),
            Err(Error {
                code: nous_core::ErrorCode::NotFound,
                ..
            }) => {}
            Err(error) => return Err(error),
        }
    }
    result.sort_by_key(ToString::to_string);
    result.dedup();
    Ok(result)
}

impl MemoryService {
    pub(super) async fn query_permitted(&self, subject: SubjectId) -> Result<bool> {
        sqlx::query_scalar("SELECT memory FROM subject_capabilities WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(nous_persistence::database_error)
            .map(|allowed| allowed.unwrap_or(false))
    }
}
