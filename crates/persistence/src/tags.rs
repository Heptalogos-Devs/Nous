//! Tag identity interpretation shared by binding and Authority projections.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_core::{CognitiveRef, TagId};

impl AuthorityStore {
    pub async fn canonical_tag_id(&self, subject: SubjectId, tag: TagId) -> Result<TagId> {
        sqlx::query_scalar::<_, Option<uuid::Uuid>>("SELECT canonical_tag($1,$2)")
            .bind(subject.0)
            .bind(tag.0)
            .fetch_one(self.pool())
            .await
            .map_err(database_error)?
            .map(TagId)
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Concept,
                    nous_core::ErrorCode::NotFound,
                    "active Tag identity not found",
                )
            })
    }
}
pub(crate) async fn canonical_topology_ref_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    reference: CognitiveRef,
) -> Result<Option<CognitiveRef>> {
    let CognitiveRef::Tag(tag) = reference else {
        return Ok(Some(reference));
    };
    Ok(
        sqlx::query_scalar::<_, Option<uuid::Uuid>>("SELECT canonical_tag($1,$2)")
            .bind(subject.0)
            .bind(tag.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)?
            .map(|id| CognitiveRef::Tag(TagId(id))),
    )
}
