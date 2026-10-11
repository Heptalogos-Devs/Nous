// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
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
        if references.len() > 256 {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Memory context batch exceeds 256 references",
            ));
        }
        let mut requested_revisions = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::MemoryRevision(id) => Some(id.0),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let object_ids = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::Memory(id) => Some(id.0),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let mut object_revision = HashMap::new();
        if !object_ids.is_empty() {
            for row in sqlx::query("SELECT memory_id,current_revision_id FROM memory_objects WHERE subject_id=$1 AND memory_id=ANY($2)")
                .bind(subject.0).bind(object_ids.iter().copied().collect::<Vec<_>>())
                .fetch_all(&mut *tx).await.map_err(db)? {
                let memory: Uuid = row.try_get("memory_id").map_err(db)?;
                let revision: Uuid = row.try_get("current_revision_id").map_err(db)?;
                requested_revisions.insert(revision);
                object_revision.insert(memory, revision);
            }
        }
        let revision_ids = requested_revisions
            .into_iter()
            .map(MemoryRevisionId)
            .collect::<Vec<_>>();
        let views = self
            .memories_by_revision_in_tx(&mut tx, subject, &revision_ids)
            .await?;
        tx.commit().await.map_err(db)?;
        let by_revision = views
            .into_iter()
            .map(|view| (view.revision.memory_revision_id.0, view))
            .collect::<HashMap<_, _>>();
        let mut result = HashMap::new();
        for reference in references {
            let revision = match reference {
                CognitiveRef::MemoryRevision(id) => Some(id.0),
                CognitiveRef::Memory(id) => object_revision.get(&id.0).copied(),
                _ => None,
            };
            if let Some(view) = revision.and_then(|revision| by_revision.get(&revision)) {
                result.insert(reference.clone(), view.clone());
            }
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
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Memory batch exceeds 256 revisions",
            ));
        }
        if revisions.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let views = self
            .memories_by_revision_in_tx(&mut tx, subject, revisions)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(views)
    }

    async fn memories_by_revision_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        revisions: &[MemoryRevisionId],
    ) -> Result<Vec<MemoryView>> {
        if revisions.len() > 256 {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Memory batch exceeds 256 revisions",
            ));
        }
        if revisions.is_empty() {
            return Ok(Vec::new());
        }
        let ids = revisions
            .iter()
            .map(|revision| revision.0)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let rows = sqlx::query("SELECT o.memory_id,o.subject_id,o.cognitive_role,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,o.accessibility_mode,o.created_at,r.memory_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.formation_mode,r.grounding_occurrence_id,r.semantic_role,r.title,r.representation_text,r.epistemic_class,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM memory_objects o JOIN memory_revisions r ON r.memory_id=o.memory_id WHERE o.subject_id=$1 AND r.subject_id=$1 AND r.memory_revision_id=ANY($2)")
            .bind(subject.0)
            .bind(&ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let mut decoded = HashMap::with_capacity(rows.len());
        let mut memory_ids = HashSet::new();
        for row in &rows {
            let (object, revision) = super::decode_memory_row(row, subject)?;
            memory_ids.insert(object.memory_id.0);
            decoded.insert(revision.memory_revision_id.0, (object, revision));
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
                basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")?,
            }));
        }
        for row in sqlx::query("SELECT memory_revision_id,target_ref_kind,target_ref,basis_role,epistemic_relation FROM memory_revision_dependencies WHERE memory_revision_id=ANY($1) ORDER BY memory_revision_id,target_ref_kind,target_ref")
            .bind(&loaded_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            basis.entry(revision).or_default().push(RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: parse_epistemic_relation(row.try_get("epistemic_relation").map_err(db)?)?,
                target_revision: parse_reference(&row.try_get::<String, _>("target_ref_kind").map_err(db)?, &row.try_get::<String, _>("target_ref").map_err(db)?)?,
                basis_role: parse_enum(row.try_get("basis_role").map_err(db)?, "basis role")?,
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
                relation: parse_enum(row.try_get("relation").map_err(db)?, "memory relation")?,
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
            temporal.entry(revision).or_insert_with(|| TemporalEvidence { occurred: Vec::new(), observed_at: None }).occurred.push(temporal_from_columns(
                row.try_get("occurred_time_kind").map_err(db)?,
                row.try_get("occurred_time_start").map_err(db)?,
                row.try_get("occurred_time_end").map_err(db)?,
            )?);
            temporal.get_mut(&revision).expect("inserted temporal evidence").observed_at = Some(row.try_get("observed_at").map_err(db)?);
            let class: String = row.try_get("source_class").map_err(db)?;
            source_classes.entry(revision).or_default().insert(class.clone(), SourceClass::from(class));
        }

        let accessibility = self
            .accessibility_levels(
                subject,
                &memory_ids.into_iter().map(MemoryId).collect::<Vec<_>>(),
                self.cognition.now(subject),
                tx,
            )
            .await?;
        let mut views = HashMap::with_capacity(decoded.len());
        for (revision_id, (object, revision)) in decoded {
            let relations = relations.remove(&revision_id).unwrap_or_default();
            let relations_truncated = relations.len() > 256;
            let basis = basis.remove(&revision_id).unwrap_or_default();
            views.insert(
                revision_id,
                MemoryView {
                    accessibility_level: *accessibility.get(&object.memory_id).ok_or_else(
                        || {
                            Error::new(
                                nous_core::ErrorOwner::Memory,
                                nous_core::ErrorCode::InternalFailure,
                                "Memory accessibility batch omitted an object",
                            )
                        },
                    )?,
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
                    object,
                    revision,
                },
            );
        }
        Ok(revisions
            .iter()
            .filter_map(|revision| views.get(&revision.0).cloned())
            .collect())
    }

    async fn accessibility_levels(
        &self,
        subject: SubjectId,
        memories: &[MemoryId],
        now: DateTime<Utc>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<HashMap<MemoryId, AccessibilityLevel>> {
        let policy = super::super::resolve_accessibility_policy(
            &self.configuration.snapshot_for_subject(subject)?,
        )?;
        let ids = memories.iter().map(|id| id.0).collect::<Vec<_>>();
        let rows = sqlx::query("SELECT o.memory_id,o.accessibility_mode,o.created_at,e.use_kind,e.occurred_at FROM memory_objects o LEFT JOIN memory_revisions r ON r.memory_id=o.memory_id LEFT JOIN cognitive_use_events e ON e.subject_id=o.subject_id AND e.ref_kind='memory_revision' AND e.ref_value=r.memory_revision_id::text AND e.use_kind<>'presented' WHERE o.subject_id=$1 AND o.memory_id=ANY($2) ORDER BY o.memory_id,e.occurred_at")
            .bind(subject.0).bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
        let mut inputs: BTreeMap<MemoryId, (AccessibilityMode, DateTime<Utc>, Vec<(String, f64)>)> =
            BTreeMap::new();
        for row in rows {
            let memory = MemoryId(row.try_get("memory_id").map_err(db)?);
            let mode = parse_enum(
                row.try_get("accessibility_mode").map_err(db)?,
                "accessibility mode",
            )?;
            let created: DateTime<Utc> = row.try_get("created_at").map_err(db)?;
            let uses = inputs.entry(memory).or_insert((mode, created, Vec::new()));
            if let Some(kind) = row.try_get::<Option<String>, _>("use_kind").map_err(db)? {
                let at: DateTime<Utc> = row.try_get("occurred_at").map_err(db)?;
                uses.2
                    .push((kind, (now - at).num_seconds().max(0) as f64 / 86400.0));
            }
        }
        inputs
            .into_iter()
            .map(|(memory, (mode, created, uses))| {
                let level =
                    match mode {
                        AccessibilityMode::Normal => AccessibilityLevel::Normal,
                        AccessibilityMode::Deep => AccessibilityLevel::Deep,
                        AccessibilityMode::Explicit => AccessibilityLevel::Explicit,
                        AccessibilityMode::Auto => policy.level_from_activation(policy.activation(
                            (now - created).num_seconds().max(0) as f64 / 86400.0,
                            &uses,
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
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InternalFailure,
            "evidence locator union is invalid",
        )),
    }
}
