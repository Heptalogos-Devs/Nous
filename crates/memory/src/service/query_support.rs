// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

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
