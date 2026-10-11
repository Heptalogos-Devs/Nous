// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;

impl MemoryService {
    pub async fn episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        revision: Option<EpisodeRevisionId>,
    ) -> Result<EpisodeView> {
        let reference = match revision {
            Some(revision) => CognitiveRef::EpisodeRevision(revision),
            None => CognitiveRef::Episode(episode),
        };
        let view = self
            .episodes_by_references(subject, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "Episode not found")
            })?;
        if view.object.episode_id != episode {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Episode not found",
            ));
        }
        Ok(view)
    }

    /// Resolve current and exact Episode references through one complete owner
    /// snapshot. Related members, basis and revision relations are loaded in
    /// grouped SQL reads, so callers never fan out per selected reference.
    pub async fn episodes_by_references(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<HashMap<CognitiveRef, EpisodeView>> {
        if references.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Episode context batch exceeds 256 references",
            ));
        }
        let episode_ids = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::Episode(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        let requested_revisions = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::EpisodeRevision(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        if episode_ids.is_empty() && requested_revisions.is_empty() {
            return Ok(HashMap::new());
        }

        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let rows = sqlx::query("SELECT o.episode_id,o.subject_id,o.track_key,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,o.created_at,r.episode_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.parent_episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end,r.boundary_explanation,r.formed_at,r.recorded_at,r.producer_signature_id FROM episode_objects o JOIN episode_revisions r ON r.episode_id=o.episode_id AND ((o.episode_id=ANY($2) AND r.episode_revision_id=o.current_revision_id) OR r.episode_revision_id=ANY($3)) WHERE o.subject_id=$1 ORDER BY o.episode_id,r.revision_no")
            .bind(subject.0)
            .bind(&episode_ids)
            .bind(&requested_revisions)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let revision_ids = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("episode_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        if revision_ids.is_empty() {
            tx.commit().await.map_err(db)?;
            return Ok(HashMap::new());
        }

        let mut members = HashMap::<Uuid, Vec<EpisodeMember>>::new();
        for row in sqlx::query("SELECT episode_revision_id,ordinal,ref_kind,ref_value,role FROM episode_revision_members WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,ordinal")
            .bind(&revision_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?
        {
            let revision: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            members.entry(revision).or_default().push(EpisodeMember {
                ordinal: row.try_get("ordinal").map_err(db)?,
                reference: parse_reference(
                    &row.try_get::<String, _>("ref_kind").map_err(db)?,
                    &row.try_get::<String, _>("ref_value").map_err(db)?,
                )?,
                role: row.try_get("role").map_err(db)?,
            });
        }

        let mut basis = HashMap::<Uuid, Vec<RevisionBasis>>::new();
        for row in sqlx::query("SELECT episode_revision_id,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM episode_revision_basis WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,basis_no")
            .bind(&revision_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?
        {
            let revision: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            basis.entry(revision).or_default().push(decode_episode_basis(&row)?);
        }

        let mut relations = HashMap::<Uuid, Vec<EpisodeRelation>>::new();
        for row in sqlx::query("WITH requested AS (SELECT unnest($1::uuid[]) AS revision_id) SELECT requested.revision_id,relation.from_revision_id,relation.to_revision_id,relation.relation,relation.created_at FROM requested JOIN episode_revision_relations relation ON relation.from_revision_id=requested.revision_id OR relation.to_revision_id=requested.revision_id ORDER BY requested.revision_id,relation.created_at,relation.from_revision_id,relation.to_revision_id")
            .bind(&revision_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?
        {
            let selected: Uuid = row.try_get("revision_id").map_err(db)?;
            relations.entry(selected).or_default().push(EpisodeRelation {
                from_revision_id: EpisodeRevisionId(row.try_get("from_revision_id").map_err(db)?),
                to_revision_id: EpisodeRevisionId(row.try_get("to_revision_id").map_err(db)?),
                relation: row.try_get("relation").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
            });
        }
        tx.commit().await.map_err(db)?;

        let mut by_episode = HashMap::<EpisodeId, EpisodeView>::new();
        let mut by_revision = HashMap::<EpisodeRevisionId, EpisodeView>::new();
        for row in rows {
            let revision_id = EpisodeRevisionId(row.try_get("episode_revision_id").map_err(db)?);
            let episode_id = EpisodeId(row.try_get("episode_id").map_err(db)?);
            let view = EpisodeView {
                object: EpisodeObject {
                    episode_id,
                    subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
                    track_key: row.try_get("track_key").map_err(db)?,
                    current_revision_id: EpisodeRevisionId(
                        row.try_get("current_revision_id").map_err(db)?,
                    ),
                    object_epoch: row.try_get("object_epoch").map_err(db)?,
                    acceptance_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("acceptance_state").map_err(db)?,
                        "acceptance state",
                    )?,
                    integrity_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("integrity_state").map_err(db)?,
                        "integrity state",
                    )?,
                    suppression_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("suppression_state").map_err(db)?,
                        "suppression state",
                    )?,
                    purge_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("purge_state").map_err(db)?,
                        "purge state",
                    )?,
                    created_at: row.try_get("created_at").map_err(db)?,
                },
                revision: EpisodeRevision {
                    episode_revision_id: revision_id,
                    episode_id,
                    subject_id: subject,
                    revision_no: row.try_get("revision_no").map_err(db)?,
                    parent_revision_id: row
                        .try_get::<Option<Uuid>, _>("parent_revision_id")
                        .map_err(db)?
                        .map(EpisodeRevisionId),
                    revision_intent: row.try_get("revision_intent").map_err(db)?,
                    title: row.try_get("title").map_err(db)?,
                    parent_episode_revision_id: row
                        .try_get::<Option<Uuid>, _>("parent_episode_revision_id")
                        .map_err(db)?
                        .map(EpisodeRevisionId),
                    experience_time: temporal_from_columns(
                        ErrorOwner::Memory,
                        row.try_get("experience_time_kind").map_err(db)?,
                        row.try_get("experience_time_start").map_err(db)?,
                        row.try_get("experience_time_end").map_err(db)?,
                    )?,
                    boundary_explanation: row.try_get("boundary_explanation").map_err(db)?,
                    formed_at: row.try_get("formed_at").map_err(db)?,
                    recorded_at: row.try_get("recorded_at").map_err(db)?,
                    producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
                },
                members: members.remove(&revision_id.0).unwrap_or_default(),
                basis: basis.remove(&revision_id.0).unwrap_or_default(),
                relations: relations.remove(&revision_id.0).unwrap_or_default(),
            };
            if view.object.current_revision_id == revision_id {
                by_episode.insert(episode_id, view.clone());
            }
            by_revision.insert(revision_id, view);
        }
        let mut result = HashMap::new();
        for reference in references {
            let view = match reference {
                CognitiveRef::Episode(id) => by_episode.get(id).cloned(),
                CognitiveRef::EpisodeRevision(id) => by_revision.get(id).cloned(),
                _ => None,
            };
            if let Some(view) = view {
                result.insert(reference.clone(), view);
            }
        }
        Ok(result)
    }

    pub async fn episode_revision(
        &self,
        subject: SubjectId,
        revision: EpisodeRevisionId,
    ) -> Result<EpisodeView> {
        let reference = CognitiveRef::EpisodeRevision(revision);
        self.episodes_by_references(subject, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(
                    ErrorOwner::Memory,
                    ErrorCode::NotFound,
                    "Episode revision not found",
                )
            })
    }

    pub async fn list_episodes(&self, subject: SubjectId) -> Result<Vec<EpisodeView>> {
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT episode_id FROM episode_objects WHERE subject_id=$1 ORDER BY created_at,episode_id")
            .bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let references = ids
            .iter()
            .copied()
            .map(|id| CognitiveRef::Episode(EpisodeId(id)))
            .collect::<Vec<_>>();
        let mut result = Vec::with_capacity(ids.len());
        for batch in references.chunks(256) {
            let views = self.episodes_by_references(subject, batch).await?;
            for reference in batch {
                result.push(views.get(reference).cloned().ok_or_else(|| {
                    Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "Episode not found")
                })?);
            }
        }
        Ok(result)
    }

    pub async fn episode_history(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
    ) -> Result<Vec<EpisodeView>> {
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT episode_revision_id FROM episode_revisions WHERE subject_id=$1 AND episode_id=$2 ORDER BY revision_no")
            .bind(subject.0).bind(episode.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = Vec::with_capacity(ids.len());
        let references = ids
            .into_iter()
            .map(|id| CognitiveRef::EpisodeRevision(EpisodeRevisionId(id)))
            .collect::<Vec<_>>();
        for batch in references.chunks(256) {
            let views = self.episodes_by_references(subject, batch).await?;
            for reference in batch {
                result.push(views.get(reference).cloned().ok_or_else(|| {
                    Error::new(
                        ErrorOwner::Memory,
                        ErrorCode::NotFound,
                        "Episode revision not found",
                    )
                })?);
            }
        }
        Ok(result)
    }
}

fn decode_episode_basis(row: &sqlx::postgres::PgRow) -> Result<RevisionBasis> {
    let role = parse_enum(
        ErrorOwner::Memory,
        row.try_get("basis_role").map_err(db)?,
        "basis role",
    )?;
    let kind: String = row.try_get("basis_kind").map_err(db)?;
    if kind == "evidence" {
        let locator = if let Some(value) = row
            .try_get::<Option<Uuid>, _>("source_region_id")
            .map_err(db)?
        {
            EvidenceLocator::SourceRegion(SourceRegionId(value))
        } else if let Some(value) = row
            .try_get::<Option<Uuid>, _>("derived_representation_id")
            .map_err(db)?
        {
            EvidenceLocator::DerivedRepresentation(DerivedRepresentationId(value))
        } else if let Some(value) = row
            .try_get::<Option<Uuid>, _>("derived_region_id")
            .map_err(db)?
        {
            EvidenceLocator::DerivedRegion(DerivedRegionId(value))
        } else {
            EvidenceLocator::WholeOccurrence
        };
        Ok(RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            occurrence_id: OccurrenceId(row.try_get::<Uuid, _>("occurrence_id").map_err(db)?),
            locator,
            basis_role: role,
        }))
    } else {
        Ok(RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            target_revision: parse_reference(
                &kind,
                &row.try_get::<String, _>("basis_ref").map_err(db)?,
            )?,
            basis_role: role,
        }))
    }
}
