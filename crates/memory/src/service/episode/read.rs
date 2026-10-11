// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::cognition::CognitionSelection;
use std::collections::HashMap;

impl MemoryService {
    pub async fn episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        revision: Option<EpisodeRevisionId>,
    ) -> Result<EpisodeView> {
        let reference = revision.map_or(
            CognitiveRef::Episode(episode),
            CognitiveRef::EpisodeRevision,
        );
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Episode(view)) if view.header.object_id() == episode.0 => Ok(view),
            _ => Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Episode not found",
            )),
        }
    }

    pub async fn episode_revision(
        &self,
        subject: SubjectId,
        revision: EpisodeRevisionId,
    ) -> Result<EpisodeView> {
        let reference = CognitiveRef::EpisodeRevision(revision);
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::Episode(view)) => Ok(view),
            _ => Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Episode revision not found",
            )),
        }
    }

    pub(in crate::service) async fn episode_views_for_selections_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        selections: &HashMap<Uuid, CognitionSelection>,
    ) -> Result<HashMap<Uuid, EpisodeView>> {
        let revision_ids = selections.keys().copied().collect::<Vec<_>>();
        if revision_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query("SELECT episode_revision_id,revision_intent,title,parent_episode_revision_id,experience_time_kind,experience_time_start,experience_time_end,boundary_explanation FROM episode_revisions WHERE episode_revision_id=ANY($1)")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)?;
        let mut members = HashMap::<Uuid, Vec<EpisodeMember>>::new();
        for row in sqlx::query("SELECT episode_revision_id,ordinal,ref_kind,ref_value,role FROM episode_revision_members WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,ordinal")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            members.entry(revision).or_default().push(EpisodeMember {
                ordinal: row.try_get("ordinal").map_err(db)?,
                reference: parse_reference(&row.try_get::<String, _>("ref_kind").map_err(db)?, &row.try_get::<String, _>("ref_value").map_err(db)?)?,
                role: row.try_get("role").map_err(db)?,
            });
        }
        let mut basis = HashMap::<Uuid, Vec<RevisionBasis>>::new();
        for row in sqlx::query("SELECT episode_revision_id,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM episode_revision_basis WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,basis_no")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let revision: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            basis
                .entry(revision)
                .or_default()
                .push(decode_revision_basis(ErrorOwner::Memory, &row)?);
        }
        let mut relations = HashMap::<Uuid, Vec<EpisodeRelation>>::new();
        for row in sqlx::query("WITH requested AS (SELECT unnest($1::uuid[]) AS revision_id) SELECT requested.revision_id,relation.from_revision_id,relation.to_revision_id,relation.relation,relation.created_at FROM requested JOIN episode_revision_relations relation ON relation.from_revision_id=requested.revision_id OR relation.to_revision_id=requested.revision_id ORDER BY requested.revision_id,relation.created_at,relation.from_revision_id,relation.to_revision_id")
            .bind(&revision_ids).fetch_all(&mut **tx).await.map_err(db)? {
            let selected: Uuid = row.try_get("revision_id").map_err(db)?;
            relations.entry(selected).or_default().push(EpisodeRelation {
                from_revision_id: EpisodeRevisionId(row.try_get("from_revision_id").map_err(db)?),
                to_revision_id: EpisodeRevisionId(row.try_get("to_revision_id").map_err(db)?),
                relation: row.try_get("relation").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
            });
        }
        let mut views = HashMap::with_capacity(rows.len());
        for row in rows {
            let revision_id: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            let Some(selection) = selections.get(&revision_id) else {
                continue;
            };
            let revision = EpisodeRevision {
                header: selection.revision.clone(),
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
            };
            views.insert(
                revision_id,
                EpisodeView {
                    header: selection.header.clone(),
                    revision,
                    members: members.remove(&revision_id).unwrap_or_default(),
                    basis: basis.remove(&revision_id).unwrap_or_default(),
                    relations: relations.remove(&revision_id).unwrap_or_default(),
                },
            );
        }
        Ok(views)
    }
}
