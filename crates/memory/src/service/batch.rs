// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;
use uuid::Uuid;

impl MemoryService {
    pub(crate) async fn memory_views(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
        accessibility_policy: &AccessibilityPolicy,
        view: Option<&HistoricalAuthoritySnapshot>,
        now: DateTime<Utc>,
    ) -> Result<HashMap<Uuid, MemoryView>> {
        let mut tx = self.store.read_view().await?;
        let result = self
            .memory_views_in(
                &mut tx,
                subject,
                (revisions, &[]),
                accessibility_policy,
                view,
                now,
            )
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one owner-level batch materializer keeps all candidate projections consistent"
    )]
    pub(in crate::service) async fn memory_views_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        selection: (&[Uuid], &[Uuid]),
        accessibility_policy: &AccessibilityPolicy,
        view: Option<&HistoricalAuthoritySnapshot>,
        now: DateTime<Utc>,
    ) -> Result<HashMap<Uuid, MemoryView>> {
        let (revisions, objects) = selection;
        if revisions.is_empty() && objects.is_empty() {
            return Ok(HashMap::new());
        }
        let now = view.map_or(now, |view| view.as_of);
        let rows = sqlx::query("SELECT o.memory_id,o.subject_id,o.cognitive_role,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.accessibility_mode,o.created_at,r.memory_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.formation_mode,r.grounding_occurrence_id,r.semantic_role,r.title,r.representation_text,r.epistemic_class,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM memory_objects o JOIN memory_revisions r ON r.memory_id=o.memory_id WHERE o.subject_id=$1 AND (r.memory_revision_id=ANY($2::uuid[]) OR (o.memory_id=ANY($3::uuid[]) AND r.memory_revision_id=o.current_revision_id))")
        .bind(subject.0)
        .bind(revisions)
        .bind(objects)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
        let selected = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("memory_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let revisions = selected.as_slice();
        let mut basis = self.memory_basis_in(tx, revisions).await?;
        let aboutness_rows = sqlx::query("SELECT memory_revision_id,entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,entity_ref")
            .bind(revisions).fetch_all(&mut **tx).await.map_err(db)?;
        let mut aboutness = HashMap::<Uuid, Vec<EntityRef>>::new();
        for row in aboutness_rows {
            let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
            aboutness.entry(revision).or_default().push(EntityRef::new(
                row.try_get::<String, _>("entity_ref").map_err(db)?,
            )?);
        }
        let occurrence_ids = basis
            .values()
            .flatten()
            .filter_map(|basis| match basis {
                RevisionBasis::Evidence(e) => Some(e.occurrence_id),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let occurrence_facts = self
            .material
            .occurrences_in(tx, subject, &occurrence_ids, view)
            .await?;
        let mut temporal = HashMap::<Uuid, TemporalEvidence>::new();
        let mut source_classes = HashMap::<Uuid, Vec<SourceClass>>::new();
        for (revision, support) in &basis {
            let mut occurrences = support
                .iter()
                .filter_map(|basis| match basis {
                    RevisionBasis::Evidence(e) => occurrence_facts.get(&e.occurrence_id),
                    _ => None,
                })
                .collect::<Vec<_>>();
            occurrences.sort_by_key(|o| o.observed_at);
            let entry = temporal.entry(*revision).or_default();
            for occurrence in occurrences {
                entry.occurred.push(occurrence.occurred_time.clone());
                {
                    entry.observed_at = Some(occurrence.observed_at);
                }
                source_classes
                    .entry(*revision)
                    .or_default()
                    .push(occurrence.source_class.clone());
            }
            if let Some(classes) = source_classes.get_mut(revision) {
                classes.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                classes.dedup();
            }
        }
        let accessibility = self
            .memory_accessibility_in(
                tx,
                subject,
                &rows
                    .iter()
                    .map(|row| row.try_get::<Uuid, _>("memory_id").map_err(db))
                    .collect::<Result<Vec<_>>>()?,
                accessibility_policy,
                view,
                now,
            )
            .await?;

        let mut tags = HashMap::<Uuid, Vec<TagId>>::new();
        let mut relations = HashMap::<Uuid, Vec<MemoryRevisionRelation>>::new();
        {
            for row in sqlx::query("SELECT memory_revision_id,tag_id FROM memory_revision_tags WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,tag_id").bind(revisions).fetch_all(&mut **tx).await.map_err(db)? {
                tags.entry(row.try_get("memory_revision_id").map_err(db)?).or_default().push(TagId(row.try_get("tag_id").map_err(db)?));
            }
            for row in sqlx::query("SELECT refs.id,r.* FROM unnest($1::uuid[]) refs(id) JOIN LATERAL (SELECT * FROM memory_revision_relations WHERE from_revision_id=refs.id OR to_revision_id=refs.id ORDER BY created_at,from_revision_id,to_revision_id LIMIT 257) r ON true ORDER BY refs.id,r.created_at,r.from_revision_id,r.to_revision_id")
                .bind(revisions).fetch_all(&mut **tx).await.map_err(db)? {
                relations.entry(row.try_get("id").map_err(db)?).or_default().push(MemoryRevisionRelation { from_revision_id: MemoryRevisionId(row.try_get("from_revision_id").map_err(db)?), to_revision_id: MemoryRevisionId(row.try_get("to_revision_id").map_err(db)?), relation: parse_enum(row.try_get("relation").map_err(db)?, "memory relation")?, created_at: row.try_get("created_at").map_err(db)? });
            }
        }
        let mut result = HashMap::new();
        for row in rows {
            let memory_id = MemoryId(row.try_get("memory_id").map_err(db)?);
            let revision_id = MemoryRevisionId(row.try_get("memory_revision_id").map_err(db)?);
            let (object, revision) = memory::decode_memory_row(&row, subject)?;
            let mut revision_relations = relations.remove(&revision_id.0).unwrap_or_default();
            let relations_truncated = revision_relations.len() > 256;
            revision_relations.truncate(256);
            result.insert(
                revision_id.0,
                MemoryView {
                    object,
                    revision,
                    basis: basis.remove(&revision_id.0).unwrap_or_default(),
                    aboutness: aboutness.remove(&revision_id.0).unwrap_or_default(),
                    tags: tags.remove(&revision_id.0).unwrap_or_default(),
                    relations: revision_relations,
                    relations_truncated,
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
