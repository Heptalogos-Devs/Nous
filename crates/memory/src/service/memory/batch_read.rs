// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::cognition::{CognitionSelection, select_cognitions_in_tx};
use chrono::{DateTime, Utc};
use nous_persistence::database_error as db;
use sqlx::Row;
use std::collections::{BTreeMap, HashMap, HashSet};

impl MemoryService {
    pub async fn memory_views_for_references(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<HashMap<CognitiveRef, MemoryView>> {
        self.memory_views_for_references_at(subject, references, self.cognition.now(subject))
            .await
    }

    /// Load the same complete batch against a caller-captured cognitive clock.
    /// Query/projection readers use this to avoid a second, live time sample.
    pub async fn memory_views_for_references_at(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        now: DateTime<Utc>,
    ) -> Result<HashMap<CognitiveRef, MemoryView>> {
        if references.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Memory context batch exceeds 256 references",
            ));
        }
        let snapshot = self.configuration.snapshot_for_subject(subject)?;
        let policy = super::super::resolve_accessibility_policy(&snapshot)?;
        self.memory_views_for_references_at_policy(subject, references, now, policy, None)
            .await
    }

    /// Load complete Memory views using the caller's frozen read policy and clock.
    pub(in crate::service) async fn memory_views_for_references_at_policy(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        now: DateTime<Utc>,
        policy: AccessibilityPolicy,
        historical_view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, MemoryView>> {
        if references.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Memory context batch exceeds 256 references",
            ));
        }
        if references.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let effective = references
            .iter()
            .filter_map(|reference| {
                let (kind, _, is_revision) = CognitionKind::from_reference(reference).ok()?;
                if kind != CognitionKind::Memory {
                    return None;
                }
                match historical_view {
                    Some(snapshot) => {
                        let state = snapshot.cognition_for(reference)?;
                        if is_revision && snapshot.selected_cognition_for(reference).is_none() {
                            return None;
                        }
                        Some(if is_revision {
                            reference.clone()
                        } else {
                            state.head.clone()
                        })
                    }
                    None => Some(reference.clone()),
                }
            })
            .collect::<Vec<_>>();
        let selected = select_cognitions_in_tx(&mut tx, subject, &effective).await?;
        let selections = unique_selections(&selected, CognitionKind::Memory);
        let views = self
            .memory_views_for_selections_in_tx(
                &mut tx,
                subject,
                &selections,
                historical_view.map_or(now, |view| view.as_of),
                policy,
                historical_view,
            )
            .await?;
        tx.commit().await.map_err(db)?;
        let mut result = HashMap::new();
        for reference in references {
            let selection = selected.get(reference).or_else(|| {
                let effective = historical_view
                    .and_then(|snapshot| snapshot.cognition_for(reference))
                    .map(|state| state.head.clone());
                effective
                    .as_ref()
                    .and_then(|effective| selected.get(effective))
            });
            let Some(mut view) = selection
                .and_then(|selection| views.get(selection.revision_ref()))
                .cloned()
            else {
                continue;
            };
            if let Some(state) =
                historical_view.and_then(|snapshot| snapshot.cognition_for(reference))
            {
                let purge = view.header.lifecycle.purge;
                view.header = crate::service::cognition::historical_cognition_header(
                    &view.header,
                    &state.state,
                    &state.head,
                    purge,
                )?;
            }
            result.insert(reference.clone(), view);
        }
        Ok(result)
    }

    /// Load complete owner views for exact revision references with bounded,
    /// batched SQL reads. Missing or foreign revisions are omitted; callers
    /// compare requested refs with the returned views and report degradation.
    pub async fn memories_by_revision(
        &self,
        subject: SubjectId,
        revisions: &[MemoryRevisionId],
    ) -> Result<Vec<MemoryView>> {
        if revisions.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Memory batch exceeds 256 revisions",
            ));
        }
        if revisions.is_empty() {
            return Ok(Vec::new());
        }
        let references = revisions
            .iter()
            .copied()
            .map(CognitiveRef::MemoryRevision)
            .collect::<Vec<_>>();
        let views = self
            .memory_views_for_references(subject, &references)
            .await?;
        Ok(revisions
            .iter()
            .filter_map(|revision| views.get(&CognitiveRef::MemoryRevision(*revision)).cloned())
            .collect())
    }

    pub(in crate::service) async fn memory_views_for_selections_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        selections: &[CognitionSelection],
        now: DateTime<Utc>,
        read_policy: AccessibilityPolicy,
        historical_view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, MemoryView>> {
        let ids = selections
            .iter()
            .map(|selection| selection.revision.revision_id())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query("SELECT memory_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end FROM memory_revisions WHERE memory_revision_id=ANY($1)")
            .bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
        let selected = selections
            .iter()
            .map(|selection| (selection.revision.revision_id(), selection))
            .collect::<HashMap<_, _>>();
        let mut decoded = HashMap::with_capacity(rows.len());
        for row in rows {
            let revision_id: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            let Some(selection) = selected.get(&revision_id) else {
                continue;
            };
            let revision = MemoryRevision {
                header: selection.revision.clone(),
                revision_intent: row
                    .try_get::<Option<String>, _>("revision_intent")
                    .map_err(db)?
                    .map(|value| parse_enum(ErrorOwner::Memory, value, "revision intent"))
                    .transpose()?,
                formation_mode: parse_enum(
                    ErrorOwner::Memory,
                    row.try_get("formation_mode").map_err(db)?,
                    "formation mode",
                )?,
                grounding_occurrence_id: row
                    .try_get::<Option<Uuid>, _>("grounding_occurrence_id")
                    .map_err(db)?
                    .map(OccurrenceId),
                semantic_role: row.try_get("semantic_role").map_err(db)?,
                title: row.try_get("title").map_err(db)?,
                representation_text: row.try_get("representation_text").map_err(db)?,
                epistemic_class: parse_enum(
                    ErrorOwner::Memory,
                    row.try_get("epistemic_class").map_err(db)?,
                    "epistemic class",
                )?,
                valid_time: temporal_from_columns(
                    ErrorOwner::Memory,
                    row.try_get("valid_time_kind").map_err(db)?,
                    row.try_get("valid_time_start").map_err(db)?,
                    row.try_get("valid_time_end").map_err(db)?,
                )?,
            };
            let header = selection.header.clone();
            decoded.insert(revision_id, (header, revision));
        }
        let loaded_ids = decoded.keys().copied().collect::<Vec<_>>();

        let mut basis: HashMap<Uuid, Vec<RevisionBasis>> = HashMap::new();
        for row in sqlx::query("SELECT memory_revision_id,occurrence_id,source_region_id,derived_representation_id,derived_region_id,basis_role,epistemic_relation FROM memory_revision_evidence WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,evidence_no")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            let locator = evidence_locator(&row)?;
            basis.entry(revision).or_default().push(RevisionBasis::Evidence(EvidenceRef {
                epistemic_relation: parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?,
                occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
                locator,
                basis_role: parse_enum(ErrorOwner::Memory, row.try_get("basis_role").map_err(db)?, "basis role")?,
            }));
        }
        for row in sqlx::query("SELECT memory_revision_id,target_ref_kind,target_ref,basis_role,epistemic_relation FROM memory_revision_dependencies WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,target_ref_kind,target_ref")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            basis.entry(revision).or_default().push(RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?,
                target_revision: parse_reference(&row.try_get::<String, _>("target_ref_kind").map_err(db)?, &row.try_get::<String, _>("target_ref").map_err(db)?)?,
                basis_role: parse_enum(ErrorOwner::Memory, row.try_get("basis_role").map_err(db)?, "basis role")?,
            }));
        }

        let mut aboutness: HashMap<Uuid, Vec<EntityRef>> = HashMap::new();
        for row in sqlx::query("SELECT memory_revision_id,entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,entity_ref")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            let entity = EntityRef::new(row.try_get::<String, _>("entity_ref").map_err(db)?)?;
            aboutness.entry(revision).or_default().push(entity);
        }
        let mut tags: HashMap<Uuid, Vec<TagId>> = HashMap::new();
        for row in sqlx::query("SELECT memory_revision_id,tag_id FROM memory_revision_tags WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,tag_id")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            tags.entry(revision).or_default().push(TagId(row.try_get("tag_id").map_err(db)?));
        }

        let mut relations: HashMap<Uuid, Vec<MemoryRevisionRelation>> = HashMap::new();
        for row in sqlx::query("WITH requested AS (SELECT unnest($1::uuid[]) AS revision_id), ranked AS (SELECT requested.revision_id AS target_revision_id, relation.from_revision_id,relation.to_revision_id,relation.relation,relation.created_at,row_number() OVER (PARTITION BY requested.revision_id ORDER BY relation.created_at,relation.from_revision_id,relation.to_revision_id) AS relation_rank FROM requested JOIN memory_revision_relations relation ON relation.from_revision_id=requested.revision_id OR relation.to_revision_id=requested.revision_id) SELECT target_revision_id,from_revision_id,to_revision_id,relation,created_at FROM ranked WHERE relation_rank<=257 ORDER BY target_revision_id,relation_rank")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let target: Uuid = row.try_get("target_revision_id").map_err(db)?;
            let relation = MemoryRevisionRelation {
                from_revision_id: MemoryRevisionId(row.try_get("from_revision_id").map_err(db)?),
                to_revision_id: MemoryRevisionId(row.try_get("to_revision_id").map_err(db)?),
                relation: parse_enum(ErrorOwner::Memory, row.try_get("relation").map_err(db)?, "memory relation")?,
                created_at: row.try_get("created_at").map_err(db)?,
            };
            if decoded.contains_key(&target) {
                let values = relations.entry(target).or_default();
                if !values.iter().any(|value| {
                    value.from_revision_id == relation.from_revision_id
                        && value.to_revision_id == relation.to_revision_id
                        && value.relation.as_str() == relation.relation.as_str()
                        && value.created_at == relation.created_at
                }) {
                    values.push(relation);
                }
            }
        }

        let mut temporal: HashMap<Uuid, TemporalEvidence> = HashMap::new();
        let mut source_classes: HashMap<Uuid, BTreeMap<String, SourceClass>> = HashMap::new();
        for row in sqlx::query("SELECT e.memory_revision_id,o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.observed_at,o.source_class FROM memory_revision_evidence e JOIN observation_occurrences o USING(occurrence_id) WHERE e.memory_revision_id=ANY($1) ORDER BY e.memory_revision_id,o.observed_at,o.occurrence_id")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            temporal.entry(revision).or_insert_with(|| TemporalEvidence { occurred: Vec::new(), observed_at: None }).occurred.push(temporal_from_columns(ErrorOwner::Memory,
                row.try_get("occurred_time_kind").map_err(db)?,
                row.try_get("occurred_time_start").map_err(db)?,
                row.try_get("occurred_time_end").map_err(db)?,
            )?);
            temporal.get_mut(&revision).expect("inserted temporal evidence").observed_at = Some(row.try_get("observed_at").map_err(db)?);
            let class: String = row.try_get("source_class").map_err(db)?;
            source_classes.entry(revision).or_default().insert(class.clone(), SourceClass::from(class));
        }

        let accessibility = self
            .accessibility_levels(subject, selections, now, read_policy, historical_view, tx)
            .await?;
        let mut views = HashMap::with_capacity(decoded.len());
        for (revision_id, (header, revision)) in decoded {
            let relations = relations.remove(&revision_id).unwrap_or_default();
            let relations_truncated = relations.len() > 256;
            let basis = basis.remove(&revision_id).unwrap_or_default();
            views.insert(
                revision_id,
                MemoryView {
                    accessibility_level: *accessibility
                        .get(&MemoryId(header.object_id()))
                        .ok_or_else(|| {
                            Error::new(
                                ErrorOwner::Memory,
                                ErrorCode::InternalFailure,
                                "Memory accessibility batch omitted an object",
                            )
                        })?,
                    temporal_evidence: temporal.remove(&revision_id).unwrap_or(TemporalEvidence {
                        occurred: Vec::new(),
                        observed_at: None,
                    }),
                    source_classes: source_classes
                        .remove(&revision_id)
                        .unwrap_or_default()
                        .into_values()
                        .collect(),
                    aboutness: aboutness.remove(&revision_id).unwrap_or_default(),
                    tags: tags.remove(&revision_id).unwrap_or_default(),
                    relations: relations.into_iter().take(256).collect(),
                    relations_truncated,
                    basis,
                    header,
                    revision,
                },
            );
        }
        Ok(views
            .into_values()
            .map(|view| (view.revision.header.revision_ref.clone(), view))
            .collect())
    }

    async fn accessibility_levels(
        &self,
        subject: SubjectId,
        selections: &[CognitionSelection],
        now: DateTime<Utc>,
        read_policy: AccessibilityPolicy,
        historical_view: Option<&HistoricalAuthoritySnapshot>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<HashMap<MemoryId, AccessibilityLevel>> {
        struct ActivationInput {
            mode: AccessibilityMode,
            created_at: DateTime<Utc>,
            uses: Vec<(String, f64)>,
        }

        let policy = read_policy;
        let current = selections
            .iter()
            .map(|selection| {
                (
                    selection.header.memory_id(),
                    (
                        selection.header.details.accessibility_mode(),
                        selection.header.created_at,
                    ),
                )
            })
            .collect::<HashMap<_, _>>();
        let ids = current.keys().map(|id| id.0).collect::<Vec<_>>();
        let rows = sqlx::query("SELECT o.object_id AS memory_id,e.use_kind,e.occurred_at FROM cognition_objects o LEFT JOIN cognition_revision_headers r ON r.subject_id=o.subject_id AND r.object_kind=o.object_kind AND r.object_id=o.object_id LEFT JOIN cognitive_use_events e ON e.subject_id=o.subject_id AND e.ref_kind='memory_revision' AND e.ref_value=r.revision_id::text AND e.use_kind<>'presented' AND e.occurred_at<=$3 WHERE o.object_kind='memory' AND o.subject_id=$1 AND o.object_id=ANY($2) ORDER BY o.object_id,e.occurred_at")
            .bind(subject.0).bind(ids).bind(now).fetch_all(&mut **tx).await.map_err(db)?;
        let mut inputs = BTreeMap::<MemoryId, ActivationInput>::new();
        for row in rows {
            let memory = MemoryId(row.try_get("memory_id").map_err(db)?);
            let (current_mode, created) = current.get(&memory).cloned().ok_or_else(|| {
                Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::InternalFailure,
                    "Memory accessibility batch omitted an object",
                )
            })?;
            let mode = historical_view
                .and_then(|view| view.cognition_for(&CognitiveRef::Memory(memory)))
                .map(|state| {
                    serde_json::from_value::<AccessibilityMode>(
                        state.state["accessibility_mode"].clone(),
                    )
                    .map_err(|error| {
                        Error::new(
                            ErrorOwner::Memory,
                            ErrorCode::InternalFailure,
                            "Could not decode historical Memory accessibility",
                        )
                        .with_source(error)
                    })
                })
                .transpose()?
                .unwrap_or(current_mode);
            let activation = inputs.entry(memory).or_insert_with(|| ActivationInput {
                mode,
                created_at: created,
                uses: Vec::new(),
            });
            if let Some(kind) = row.try_get::<Option<String>, _>("use_kind").map_err(db)? {
                let at: DateTime<Utc> = row.try_get("occurred_at").map_err(db)?;
                activation
                    .uses
                    .push((kind, (now - at).num_seconds().max(0) as f64 / 86400.0));
            }
        }
        inputs
            .into_iter()
            .map(|(memory, activation)| {
                let level = match activation.mode {
                    AccessibilityMode::Normal => AccessibilityLevel::Normal,
                    AccessibilityMode::Deep => AccessibilityLevel::Deep,
                    AccessibilityMode::Explicit => AccessibilityLevel::Explicit,
                    AccessibilityMode::Auto => policy.level_from_activation(policy.activation(
                        (now - activation.created_at).num_seconds().max(0) as f64 / 86400.0,
                        &activation.uses,
                    )),
                };
                Ok((memory, level))
            })
            .collect()
    }
}

fn evidence_locator(row: &sqlx::postgres::PgRow) -> Result<EvidenceLocator> {
    match (
        row.try_get::<Option<Uuid>, _>("source_region_id")
            .map_err(db)?,
        row.try_get::<Option<Uuid>, _>("derived_representation_id")
            .map_err(db)?,
        row.try_get::<Option<Uuid>, _>("derived_region_id")
            .map_err(db)?,
    ) {
        (Some(id), None, None) => Ok(EvidenceLocator::SourceRegion(SourceRegionId(id))),
        (None, Some(id), None) => Ok(EvidenceLocator::DerivedRepresentation(
            DerivedRepresentationId(id),
        )),
        (None, None, Some(id)) => Ok(EvidenceLocator::DerivedRegion(DerivedRegionId(id))),
        (None, None, None) => Ok(EvidenceLocator::WholeOccurrence),
        _ => Err(Error::new(
            ErrorOwner::Memory,
            ErrorCode::InternalFailure,
            "Evidence locator union is invalid",
        )),
    }
}

fn unique_selections(
    selected: &HashMap<CognitiveRef, CognitionSelection>,
    kind: CognitionKind,
) -> Vec<CognitionSelection> {
    selected
        .values()
        .filter(|selection| selection.header.kind() == kind)
        .map(|selection| (selection.revision.revision_ref.clone(), selection.clone()))
        .collect::<HashMap<_, _>>()
        .into_values()
        .collect()
}
