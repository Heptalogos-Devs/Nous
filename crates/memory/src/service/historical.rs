//! Memory owns historical cognition and concept semantics over canonical chronology.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
impl MemoryService {
    pub async fn project_as_of(
        &self,
        subject: SubjectId,
        as_of: DateTime<Utc>,
        revision_view: RevisionView,
    ) -> Result<HistoricalAuthoritySnapshot> {
        self.store.require_subject(subject).await?;
        let rows = self.store.historical_owner_rows(subject, as_of).await?;
        let mut cognition = Vec::new();
        for mut row in rows.cognition {
            let object = parse_reference(&row.object_kind, &row.object_id.to_string())?;
            let kind = format!("{}_revision", row.object_kind);
            let mut revisions = row
                .revisions
                .into_iter()
                .map(|id| parse_reference(&kind, &id.to_string()))
                .collect::<Result<Vec<_>>>()?;
            let head = revisions.first().cloned().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InternalFailure,
                    "historical cognition missing effective revision",
                )
            })?;
            row.state["current_revision_id"] = serde_json::json!(reference_parts(&head).1);
            let recorded_revisions = revisions.clone();
            if revision_view == RevisionView::Current {
                revisions.truncate(1);
            }
            cognition.push(HistoricalCognitionState {
                object,
                head,
                revisions,
                recorded_revisions,
                state: row.state,
            });
        }
        let tags = rows
            .tags
            .into_iter()
            .map(|row| {
                let canonical_tag = row.state["canonical_tag_id"]
                    .as_str()
                    .map(str::parse::<Uuid>)
                    .transpose()
                    .map_err(|error| {
                        Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InternalFailure,
                            "invalid canonical Tag state",
                        )
                        .with_source(error)
                    })?
                    .map(TagId);
                Ok(HistoricalTagState {
                    tag: row.tag_id,
                    revision_id: row.revision_id,
                    canonical_tag,
                    status: row.state["status"]
                        .as_str()
                        .ok_or_else(|| {
                            Error::new(
                                nous_core::ErrorOwner::Memory,
                                nous_core::ErrorCode::InternalFailure,
                                "Tag state missing status",
                            )
                        })?
                        .into(),
                    semantic: tag_semantic_representation(
                        &row.label,
                        row.description.as_deref(),
                        row.kind_hint.as_deref(),
                    )?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut snapshot = HistoricalAuthoritySnapshot::empty(subject, as_of, revision_view);
        snapshot.cognition = cognition;
        snapshot.tags = tags;
        snapshot.associations = rows.associations;
        snapshot.schema_evidence_links = rows.schema_evidence_links;
        snapshot.entity_bindings = rows.entity_bindings;
        snapshot.material_documents = rows.material.clone();
        snapshot.material_visibility = rows.material;
        snapshot.lexical_visibility = rows.lexical;
        snapshot.refresh_digest()?;
        Ok(snapshot)
    }
}
