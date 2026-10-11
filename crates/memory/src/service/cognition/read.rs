//! Typed shared Cognition reads and history selection.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::{AccessibilityQueryPolicy, AuthorityReadView, ReadContext};
use std::collections::HashMap;

#[derive(Clone)]
pub(in crate::service) struct CognitionSelection {
    pub header: CognitionHeader,
    pub revision: CognitionRevisionHeader,
}

/// Select current heads and exact revisions once for a complete owner read.
/// Family loaders receive this shared state and only decode their typed payload.
async fn select_cognitions_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    references: &[CognitiveRef],
) -> Result<HashMap<CognitiveRef, CognitionSelection>> {
    if references.is_empty() {
        return Ok(HashMap::new());
    }
    let mut objects = HashMap::<(String, Uuid), ()>::new();
    let mut revisions = HashMap::<(String, Uuid), ()>::new();
    for reference in references {
        let (kind, id, is_revision) = CognitionKind::from_reference(reference)?;
        if is_revision {
            revisions.insert((kind.as_str().to_owned(), id), ());
        } else {
            objects.insert((kind.as_str().to_owned(), id), ());
        }
    }
    let (object_kinds, object_ids): (Vec<_>, Vec<_>) = objects.into_keys().unzip();
    let (revision_kinds, revision_ids): (Vec<_>, Vec<_>) = revisions.into_keys().unzip();
    let rows = sqlx::query("SELECT o.object_kind,o.object_id,o.subject_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,o.created_at,o.cognitive_role,o.accessibility_mode,o.track_key,h.revision_id,h.revision_no,h.parent_revision_id,h.formed_at,h.recorded_at,h.producer_signature_id FROM cognition_objects o JOIN cognition_revision_headers h ON h.subject_id=o.subject_id AND h.object_kind=o.object_kind AND h.object_id=o.object_id WHERE o.subject_id=$1 AND ((h.revision_id=o.current_revision_id AND EXISTS(SELECT 1 FROM unnest($2::text[],$3::uuid[]) requested(kind,id) WHERE requested.kind=o.object_kind AND requested.id=o.object_id)) OR EXISTS(SELECT 1 FROM unnest($4::text[],$5::uuid[]) requested(kind,id) WHERE requested.kind=o.object_kind AND requested.id=h.revision_id))")
        .bind(subject.0)
        .bind(object_kinds)
        .bind(object_ids)
        .bind(revision_kinds)
        .bind(revision_ids)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    let mut by_revision = HashMap::with_capacity(rows.len());
    let mut by_object = HashMap::with_capacity(rows.len());
    for row in &rows {
        let kind = match row
            .try_get::<String, _>("object_kind")
            .map_err(db)?
            .as_str()
        {
            "memory" => CognitionKind::Memory,
            "cognitive_schema" => CognitionKind::CognitiveSchema,
            "episode" => CognitionKind::Episode,
            "journal" => CognitionKind::Journal,
            _ => {
                return Err(Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InternalFailure,
                    "invalid Cognition kind in Authority",
                ));
            }
        };
        let object_id: Uuid = row.try_get("object_id").map_err(db)?;
        let revision_id: Uuid = row.try_get("revision_id").map_err(db)?;
        let current_revision_id: Uuid = row.try_get("current_revision_id").map_err(db)?;
        let details = match kind {
            CognitionKind::Memory => CognitionDetails::Memory {
                role: parse_enum(
                    ErrorOwner::Memory,
                    row.try_get("cognitive_role").map_err(db)?,
                    "cognitive role",
                )?,
                accessibility: parse_enum(
                    ErrorOwner::Memory,
                    row.try_get("accessibility_mode").map_err(db)?,
                    "accessibility mode",
                )?,
            },
            CognitionKind::CognitiveSchema => CognitionDetails::CognitiveSchema,
            CognitionKind::Episode => CognitionDetails::Episode {
                track_key: row.try_get("track_key").map_err(db)?,
            },
            CognitionKind::Journal => CognitionDetails::Journal,
        };
        let selection = CognitionSelection {
            header: decode_cognition_header(row, kind, object_id, current_revision_id, details)?,
            revision: decode_cognition_revision_header(row, kind, object_id, revision_id)?,
        };
        if selection.header.current_revision_id() == revision_id {
            by_object.insert(selection.header.object_ref.clone(), selection.clone());
        }
        by_revision.insert(selection.revision.revision_ref.clone(), selection);
    }
    let mut selected = HashMap::with_capacity(references.len());
    for reference in references {
        let (_, _, is_revision) = CognitionKind::from_reference(reference)?;
        let selection = if is_revision {
            by_revision.get(reference)
        } else {
            by_object.get(reference)
        };
        if let Some(selection) = selection {
            selected.insert(reference.clone(), selection.clone());
        }
    }
    Ok(selected)
}

impl MemoryService {
    /// Capture the same current read contract used by projections and queries.
    pub async fn current_read_context(&self, subject: SubjectId) -> Result<ReadContext> {
        use nous_core::{AuthorityView, CognitiveEffort, RevisionView, TemporalFrame};
        let stamp = self.store.read_context_stamp(subject).await?;
        let config_snapshot = self.configuration.snapshot_for_subject(subject)?;
        Ok(ReadContext {
            subject,
            view: AuthorityReadView::Current {
                bound_authority_seq: stamp.authority_seq,
            },
            temporal: TemporalFrame {
                clock_now: self.cognition.now(subject),
                authority_view: AuthorityView::Current,
                revision_view: RevisionView::Current,
                original_expressions: Vec::new(),
            },
            accessibility: AccessibilityQueryPolicy {
                effort: CognitiveEffort::Normal,
                exact_target_bypasses_auto_level: false,
            },
            config_snapshot: std::sync::Arc::new(config_snapshot),
            purge_epoch: stamp.serving_purge_epoch,
        })
    }

    /// Resolve current or historically frozen refs, select shared headers once,
    /// then load only each family’s typed payload and evidence.
    pub async fn read_cognitions(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
    ) -> Result<HashMap<CognitiveRef, CognitionView>> {
        let historical = context.historical_view();
        let mut requested = Vec::with_capacity(references.len());
        for reference in references {
            let (kind, _, is_revision) = CognitionKind::from_reference(reference)?;
            let effective = if let Some(snapshot) = historical {
                let Some(state) = snapshot.cognition_for(reference) else {
                    continue;
                };
                if is_revision && snapshot.selected_cognition_for(reference).is_none() {
                    continue;
                }
                if is_revision {
                    reference.clone()
                } else {
                    state.head.clone()
                }
            } else {
                reference.clone()
            };
            requested.push((reference.clone(), effective, kind));
        }
        if requested.is_empty() {
            return Ok(HashMap::new());
        }
        let effective_refs = requested
            .iter()
            .map(|(_, reference, _)| reference.clone())
            .collect::<Vec<_>>();
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let selections = select_cognitions_in_tx(&mut tx, context.subject, &effective_refs).await?;
        let memories = unique_selections(&selections, CognitionKind::Memory);
        let schemas = unique_selections(&selections, CognitionKind::CognitiveSchema);
        let episodes = unique_selections(&selections, CognitionKind::Episode);
        let journals = unique_selections(&selections, CognitionKind::Journal);
        let policy = resolve_accessibility_policy(context.config_snapshot.as_ref())?;
        // Historical activation only sees uses and age at the frozen cutoff.
        let memory_clock = historical.map_or(context.temporal.clock_now, |view| view.as_of);
        let memory_views = self
            .memory_views_for_selections_in_tx(
                &mut tx,
                context.subject,
                &memories,
                memory_clock,
                policy,
                historical,
            )
            .await?;
        let schema_views = self
            .schema_views_for_selections_in_tx(&mut tx, context.subject, &schemas, historical)
            .await?;
        let episode_views = self
            .episode_views_for_selections_in_tx(&mut tx, &episodes)
            .await?;
        let journal_views = self
            .journal_views_for_selections_in_tx(&mut tx, &journals)
            .await?;
        tx.commit().await.map_err(db)?;

        let mut result = HashMap::with_capacity(references.len());
        for (original, effective, kind) in requested {
            let Some(selection) = selections.get(&effective) else {
                continue;
            };
            let revision_id = selection.revision.revision_id();
            let view = match kind {
                CognitionKind::Memory => memory_views
                    .get(&revision_id)
                    .cloned()
                    .map(CognitionView::Memory),
                CognitionKind::CognitiveSchema => schema_views
                    .get(&revision_id)
                    .cloned()
                    .map(CognitionView::CognitiveSchema),
                CognitionKind::Episode => episode_views
                    .get(&revision_id)
                    .cloned()
                    .map(CognitionView::Episode),
                CognitionKind::Journal => journal_views
                    .get(&revision_id)
                    .cloned()
                    .map(CognitionView::Journal),
            };
            if let Some(mut view) = view {
                if let Some(state) =
                    historical.and_then(|snapshot| snapshot.cognition_for(&original))
                {
                    let current_purge = view.header().lifecycle.purge;
                    let header = historical_cognition_header(
                        view.header(),
                        &state.state,
                        &state.head,
                        current_purge,
                    )?;
                    match &mut view {
                        CognitionView::Memory(value) => value.header = header,
                        CognitionView::CognitiveSchema(value) => value.header = header,
                        CognitionView::Episode(value) => value.header = header,
                        CognitionView::Journal(value) => value.header = header,
                    }
                }
                result.insert(original, view);
            }
        }
        Ok(result)
    }

    pub async fn read_cognition(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
    ) -> Result<CognitionView> {
        let (kind, _, _) = CognitionKind::from_reference(&reference)?;
        let context = self.current_read_context(subject).await?;
        self.read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(
                    cognition_owner(kind),
                    ErrorCode::NotFound,
                    "Cognition reference not found",
                )
            })
    }

    /// Materialize cognition results at the response boundary. An immutable
    /// revision recorded beside an object reference always selects that exact
    /// revision; object-only results select the current view.
    pub async fn read_mutation_cognitions(
        &self,
        subject: SubjectId,
        results: &[MutationResult],
    ) -> Result<Vec<CognitionView>> {
        let mut references = Vec::with_capacity(results.len());
        for result in results {
            let (kind, _, is_revision) = CognitionKind::from_reference(&result.reference)?;
            let reference = match (is_revision, result.revision_id) {
                (true, Some(_)) => {
                    return Err(Error::new(
                        cognition_owner(kind),
                        ErrorCode::InternalFailure,
                        "mutation result contains both an exact revision reference and a revision id",
                    ));
                }
                (true, None) => result.reference.clone(),
                (false, Some(revision)) => revision_ref(kind, revision),
                (false, None) => result.reference.clone(),
            };
            references.push(reference);
        }

        let context = self.current_read_context(subject).await?;
        let mut views = self.read_cognitions(&context, &references).await?;
        results
            .iter()
            .zip(references)
            .map(|(result, reference)| {
                let (kind, _, _) = CognitionKind::from_reference(&result.reference)?;
                views.remove(&reference).ok_or_else(|| {
                    Error::new(
                        cognition_owner(kind),
                        ErrorCode::NotFound,
                        "cognition mutation result is no longer readable",
                    )
                })
            })
            .collect()
    }

    /// Return object refs in the common keyset order used by all four family
    /// list commands. The caller retains its kind-specific page-token scope.
    pub async fn list_cognition_refs(
        &self,
        subject: SubjectId,
        kind: CognitionKind,
        status: &str,
        after: Option<Uuid>,
        limit: i64,
    ) -> Result<Vec<CognitiveRef>> {
        if !(1..=201).contains(&limit) || !["", "accepted", "withdrawn"].contains(&status) {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "invalid Cognition page",
            ));
        }
        self.store.require_subject(subject).await?;
        let ids = sqlx::query_scalar::<_, Uuid>(
            "SELECT object_id FROM cognition_objects WHERE subject_id=$1 AND object_kind=$2 AND ($3='' OR acceptance_state=$3) AND ($4::uuid IS NULL OR object_id>$4) ORDER BY object_id LIMIT $5",
        )
        .bind(subject.0)
        .bind(kind.as_str())
        .bind(status)
        .bind(after)
        .bind(limit)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        Ok(ids.into_iter().map(|id| object_ref(kind, id)).collect())
    }

    /// Return exact revision refs after an owner-local revision cursor.
    pub async fn cognition_history_refs(
        &self,
        subject: SubjectId,
        object_ref: &CognitiveRef,
        after: Option<&CognitiveRef>,
        limit: i64,
    ) -> Result<Vec<CognitiveRef>> {
        if !(1..=201).contains(&limit) {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "invalid Cognition history limit",
            ));
        }
        let (kind, object_id, is_revision) = CognitionKind::from_reference(object_ref)?;
        let error_owner = cognition_owner(kind);
        if is_revision {
            return Err(Error::new(
                error_owner,
                ErrorCode::InvalidRequest,
                "history target must be an object reference",
            ));
        }
        let after_no = if let Some(after) = after {
            let (after_kind, revision_id, is_revision) = CognitionKind::from_reference(after)?;
            if !is_revision || after_kind != kind {
                return Err(Error::new(
                    error_owner,
                    ErrorCode::InvalidRequest,
                    "history cursor kind mismatch",
                ));
            }
            sqlx::query_scalar::<_, i32>("SELECT revision_no FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 AND revision_id=$4")
                .bind(subject.0).bind(kind.as_str()).bind(object_id).bind(revision_id)
                .fetch_optional(self.store.pool()).await.map_err(db)?
                .ok_or_else(|| Error::new(ErrorOwner::Memory, ErrorCode::InvalidRequest, "history cursor does not belong to this object"))?
        } else {
            0
        };
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind=$2 AND object_id=$3 AND revision_no>$4 ORDER BY revision_no LIMIT $5")
            .bind(subject.0).bind(kind.as_str()).bind(object_id).bind(after_no).bind(limit)
            .fetch_all(self.store.pool()).await.map_err(db)?;
        if ids.is_empty() {
            self.read_cognition(subject, object_ref.clone()).await?;
        }
        Ok(ids.into_iter().map(|id| revision_ref(kind, id)).collect())
    }
}

fn historical_cognition_header(
    current: &CognitionHeader,
    state: &serde_json::Value,
    head: &CognitiveRef,
    current_purge: PurgeState,
) -> Result<CognitionHeader> {
    fn parse<T: serde::de::DeserializeOwned>(value: &serde_json::Value, field: &str) -> Result<T> {
        serde_json::from_value(value[field].clone()).map_err(|error| {
            Error::new(
                ErrorOwner::Memory,
                ErrorCode::InternalFailure,
                "historical Cognition header is invalid",
            )
            .with_context("field", field)
            .with_source(error)
        })
    }
    let mut lifecycle = CognitionLifecycle {
        acceptance: parse(state, "acceptance_state")?,
        integrity: parse(state, "integrity_state")?,
        suppression: parse(state, "suppression_state")?,
        purge: parse(state, "purge_state")?,
    };
    lifecycle.purge = current_purge;
    let details = match current.kind() {
        CognitionKind::Memory => CognitionDetails::Memory {
            role: parse(state, "cognitive_role")?,
            accessibility: parse(state, "accessibility_mode")?,
        },
        CognitionKind::CognitiveSchema => CognitionDetails::CognitiveSchema,
        CognitionKind::Episode => CognitionDetails::Episode {
            track_key: state["track_key"]
                .as_str()
                .unwrap_or(current.details.track_key())
                .to_owned(),
        },
        CognitionKind::Journal => CognitionDetails::Journal,
    };
    let created_at = parse(state, "created_at")?;
    let (_, head_id, is_revision) = CognitionKind::from_reference(head)?;
    debug_assert!(is_revision);
    Ok(CognitionHeader::new(
        current.kind(),
        current.subject_id,
        current.object_id(),
        head_id,
        state["object_epoch"].as_i64().ok_or_else(|| {
            Error::new(
                ErrorOwner::Memory,
                ErrorCode::InternalFailure,
                "historical object epoch missing",
            )
        })?,
        lifecycle,
        created_at,
        details,
    ))
}

fn unique_selections(
    selections: &HashMap<CognitiveRef, CognitionSelection>,
    kind: CognitionKind,
) -> HashMap<Uuid, CognitionSelection> {
    selections
        .values()
        .filter(|selection| selection.header.kind() == kind)
        .map(|selection| (selection.revision.revision_id(), selection.clone()))
        .collect()
}
