//! Consume the shared SourceRead view; this owner has no independent permission graph.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl MotivationService {
    pub(crate) async fn references_allowed_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        view: &SourceReadView,
        references: &[(CognitiveRef, bool)],
    ) -> Result<bool> {
        for (reference, metadata_only) in references {
            let allowed = if *metadata_only {
                let (kind, value) = reference_parts(reference);
                sqlx::query_scalar("SELECT source_read_metadata_allowed($1,$2,$3,$4,$5,$6,$7)")
                    .bind(view.subject.0)
                    .bind(kind)
                    .bind(value)
                    .bind(&view.root_keys)
                    .bind(&view.work_contexts)
                    .bind(&view.origin_conversation_refs)
                    .bind(serde_json::json!(&view.explicit_refs))
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(db)?
            } else {
                AuthorityStore::source_read_allowed_in(tx, view, reference).await?
            };
            if !allowed {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
