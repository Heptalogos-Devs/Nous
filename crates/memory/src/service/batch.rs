// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;
use uuid::Uuid;

impl MemoryService {
    /// Materialize the selected revision set through owner-batched Authority
    /// reads. Query orchestration must not turn a candidate list into one
    /// `memory(...)` call per candidate.
    #[expect(
        clippy::too_many_lines,
        reason = "one owner-level batch materializer keeps all candidate projections consistent"
    )]
    pub(crate) async fn memories_for_query(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
        accessibility_policy: &AccessibilityPolicy,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<Uuid, MemoryView>> {
        if revisions.is_empty() {
            return Ok(HashMap::new());
        }
        let now = view.map_or_else(|| self.cognition.now(subject), |view| view.as_of);
        let rows = sqlx::query("SELECT o.memory_id,o.subject_id,o.cognitive_role,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.accessibility_mode,o.created_at,r.memory_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.formation_mode,r.grounding_occurrence_id,r.semantic_role,r.title,r.representation_text,r.epistemic_class,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM memory_objects o JOIN memory_revisions r ON r.memory_id=o.memory_id WHERE o.subject_id=$1 AND r.memory_revision_id=ANY($2::uuid[])")
        .bind(subject.0)
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let evidence_rows = sqlx::query("SELECT memory_revision_id,occurrence_id,source_region_id,derived_representation_id,derived_region_id,basis_role,evidence_no,epistemic_relation FROM memory_revision_evidence WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,evidence_no")
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let dependency_rows = sqlx::query("SELECT memory_revision_id,target_ref_kind,target_ref,basis_role,epistemic_relation FROM memory_revision_dependencies WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,target_ref_kind,target_ref")
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let aboutness_rows = sqlx::query("SELECT memory_revision_id,entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,entity_ref")
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let temporal_rows = sqlx::query("SELECT e.memory_revision_id,o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.observed_at FROM memory_revision_evidence e JOIN observation_occurrences o USING(occurrence_id) WHERE e.memory_revision_id=ANY($1::uuid[]) ORDER BY e.memory_revision_id,o.observed_at")
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let source_rows = sqlx::query("SELECT DISTINCT e.memory_revision_id,o.source_class FROM memory_revision_evidence e JOIN observation_occurrences o USING(occurrence_id) WHERE e.memory_revision_id=ANY($1::uuid[]) ORDER BY e.memory_revision_id,o.source_class")
        .bind(revisions)
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;

        let mut basis = HashMap::<Uuid, Vec<RevisionBasis>>::new();
        for row in evidence_rows {
            let locator = match (
                row.try_get::<Option<Uuid>, _>("source_region_id")
                    .map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_representation_id")
                    .map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_region_id")
                    .map_err(db)?,
            ) {
                (Some(value), None, None) => EvidenceLocator::SourceRegion(SourceRegionId(value)),
                (None, Some(value), None) => {
                    EvidenceLocator::DerivedRepresentation(DerivedRepresentationId(value))
                }
                (None, None, Some(value)) => EvidenceLocator::DerivedRegion(DerivedRegionId(value)),
                (None, None, None) => EvidenceLocator::WholeOccurrence,
                _ => {
                    return Err(Error::Infrastructure(
                        "evidence locator union is invalid".into(),
                    ));
                }
            };
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            basis
                .entry(revision)
                .or_default()
                .push(RevisionBasis::Evidence(EvidenceRef {
                    epistemic_relation: parse_epistemic_relation(
                        row.try_get("epistemic_relation").map_err(db)?,
                    )?,
                    occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
                    locator,
                    basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")?,
                }));
        }
        for row in dependency_rows {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            basis
                .entry(revision)
                .or_default()
                .push(RevisionBasis::CognitionDependency(CognitionDependency {
                    epistemic_relation: parse_epistemic_relation(
                        row.try_get("epistemic_relation").map_err(db)?,
                    )?,
                    target_revision: parse_reference(
                        &row.try_get::<String, _>("target_ref_kind").map_err(db)?,
                        &row.try_get::<String, _>("target_ref").map_err(db)?,
                    )?,
                    basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")?,
                }));
        }
        let mut aboutness = HashMap::<Uuid, Vec<EntityRef>>::new();
        for row in aboutness_rows {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            aboutness.entry(revision).or_default().push(EntityRef::new(
                row.try_get::<String, _>("entity_ref").map_err(db)?,
            )?);
        }
        let mut temporal = HashMap::<Uuid, TemporalEvidence>::new();
        for row in temporal_rows {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            let entry = temporal.entry(revision).or_default();
            entry.occurred.push(temporal_from_columns(
                row.try_get("occurred_time_kind").map_err(db)?,
                row.try_get("occurred_time_start").map_err(db)?,
                row.try_get("occurred_time_end").map_err(db)?,
            )?);
            if entry.observed_at.is_none() {
                entry.observed_at = row.try_get("observed_at").map_err(db)?;
            }
        }
        let mut source_classes = HashMap::<Uuid, Vec<SourceClass>>::new();
        for row in source_rows {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            source_classes
                .entry(revision)
                .or_default()
                .push(SourceClass::from(
                    row.try_get::<String, _>("source_class").map_err(db)?,
                ));
        }
        let memory_ids = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("memory_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let accessibility_rows = sqlx::query("SELECT o.memory_id,o.accessibility_mode,o.created_at,u.use_kind,u.occurred_at FROM memory_objects o LEFT JOIN cognitive_use_events u ON u.subject_id=o.subject_id AND u.ref_kind='memory_revision' AND u.ref_value IN (SELECT memory_revision_id::text FROM memory_revisions mr2 WHERE mr2.memory_id=o.memory_id) AND u.use_kind <> 'presented' AND ($3::timestamptz IS NULL OR u.occurred_at<=$3) WHERE o.subject_id=$1 AND o.memory_id=ANY($2::uuid[])")
        .bind(subject.0)
        .bind(&memory_ids)
        .bind(view.map(|v|v.as_of))
        .fetch_all(self.store.pool())
        .await
        .map_err(db)?;
        let mut accessibility = HashMap::<Uuid, AccessibilityLevel>::new();
        let mut accessibility_inputs =
            HashMap::<Uuid, (String, DateTime<Utc>, Vec<(String, f64)>)>::new();
        for row in accessibility_rows {
            let memory: Uuid = row.try_get("memory_id").map_err(db)?;
            let entry = accessibility_inputs.entry(memory).or_insert_with(|| {
                (
                    row.try_get("accessibility_mode")
                        .unwrap_or_else(|_| "auto".into()),
                    row.try_get("created_at").unwrap_or(now),
                    Vec::new(),
                )
            });
            if let (Some(kind), Some(at)) = (
                row.try_get::<Option<String>, _>("use_kind").map_err(db)?,
                row.try_get::<Option<DateTime<Utc>>, _>("occurred_at")
                    .map_err(db)?,
            ) {
                entry
                    .2
                    .push((kind, (now - at).num_seconds().max(0) as f64 / 86400.0));
            }
        }
        for (memory, (mut mode, created, uses)) in accessibility_inputs {
            if let Some(state) =
                view.and_then(|v| v.cognition_for(&CognitiveRef::Memory(MemoryId(memory))))
            {
                mode = state.state["accessibility_mode"]
                    .as_str()
                    .unwrap_or("auto")
                    .to_owned();
            }
            let level = match mode.as_str() {
                "normal" => AccessibilityLevel::Normal,
                "deep" => AccessibilityLevel::Deep,
                "explicit" => AccessibilityLevel::Explicit,
                _ => accessibility_policy.level_from_activation(
                    accessibility_policy
                        .activation((now - created).num_seconds().max(0) as f64 / 86400.0, &uses),
                ),
            };
            accessibility.insert(memory, level);
        }

        let mut result = HashMap::new();
        for row in rows {
            let memory_id = MemoryId(row.try_get("memory_id").map_err(db)?);
            let revision_id = MemoryRevisionId(row.try_get("memory_revision_id").map_err(db)?);
            let (object, revision) = memory::decode_memory_row(&row, subject)?;
            result.insert(
                revision_id.0,
                MemoryView {
                    object,
                    revision,
                    basis: basis.remove(&revision_id.0).unwrap_or_default(),
                    aboutness: aboutness.remove(&revision_id.0).unwrap_or_default(),
                    tags: Vec::new(),
                    relations: Vec::new(),
                    relations_truncated: false,
                    accessibility_level: accessibility
                        .get(&memory_id.0)
                        .copied()
                        .unwrap_or(AccessibilityLevel::Normal),
                    temporal_evidence: temporal.remove(&revision_id.0).unwrap_or_default(),
                    source_classes: source_classes.remove(&revision_id.0).unwrap_or_default(),
                },
            );
        }
        Ok(result)
    }
}
