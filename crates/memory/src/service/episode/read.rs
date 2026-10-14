// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        revision: Option<EpisodeRevisionId>,
    ) -> Result<EpisodeView> {
        let mut tx = self.store.read_view().await?;
        let revisions = revision.map(|id| vec![id.0]).unwrap_or_default();
        let objects = if revision.is_none() {
            vec![episode.0]
        } else {
            vec![]
        };
        let views = self
            .episode_views_in(&mut tx, subject, &revisions, &objects)
            .await?;
        tx.commit().await.map_err(db)?;
        views
            .into_values()
            .find(|view| view.object.episode_id == episode)
            .ok_or_else(|| Error::NotFound("Episode not found".into()))
    }

    pub(in crate::service) async fn episode_views(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, EpisodeView>> {
        let mut tx = self.store.read_view().await?;
        let result = self
            .episode_views_in(&mut tx, subject, revisions, &[])
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(result)
    }

    pub(in crate::service) async fn episode_views_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        revisions: &[Uuid],
        objects: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, EpisodeView>> {
        use std::collections::HashMap;
        let rows = sqlx::query("SELECT o.episode_id,o.subject_id,o.track_key,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.created_at,r.episode_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.parent_episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end,r.boundary_explanation,r.formed_at,r.recorded_at,r.producer_signature_id FROM episode_objects o JOIN episode_revisions r ON r.episode_id=o.episode_id WHERE o.subject_id=$1 AND (r.episode_revision_id=ANY($2) OR (o.episode_id=ANY($3) AND r.episode_revision_id=o.current_revision_id))").bind(subject.0).bind(revisions).bind(objects).fetch_all(&mut **tx).await.map_err(db)?;
        let selected = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("episode_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let (mut basis, mut members) = self.episode_supports_in(tx, &selected).await?;
        let mut relations: HashMap<Uuid, Vec<EpisodeRelation>> = HashMap::new();
        for row in sqlx::query("SELECT from_revision_id,to_revision_id,relation,created_at FROM episode_revision_relations WHERE from_revision_id=ANY($1) OR to_revision_id=ANY($1) ORDER BY created_at,from_revision_id,to_revision_id").bind(&selected).fetch_all(&mut **tx).await.map_err(db)? {
            let relation = EpisodeRelation {
                from_revision_id: EpisodeRevisionId(row.try_get("from_revision_id").map_err(db)?),
                to_revision_id: EpisodeRevisionId(row.try_get("to_revision_id").map_err(db)?),
                relation: row.try_get("relation").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
            };
            for id in [relation.from_revision_id.0, relation.to_revision_id.0] {
                if selected.contains(&id) { relations.entry(id).or_default().push(relation.clone()); }
            }
        }
        rows.into_iter()
            .map(|row| {
                let revision_id: Uuid = row.try_get("episode_revision_id").map_err(db)?;
                Ok(EpisodeView {
                    object: EpisodeObject {
                        episode_id: EpisodeId(row.try_get("episode_id").map_err(db)?),
                        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
                        track_key: row.try_get("track_key").map_err(db)?,
                        current_revision_id: EpisodeRevisionId(
                            row.try_get("current_revision_id").map_err(db)?,
                        ),
                        object_epoch: row.try_get("object_epoch").map_err(db)?,
                        acceptance_state: parse_enum(
                            row.try_get("acceptance_state").map_err(db)?,
                            "acceptance state",
                        )?,
                        integrity_state: parse_enum(
                            row.try_get("integrity_state").map_err(db)?,
                            "integrity state",
                        )?,
                        suppression_state: parse_enum(
                            row.try_get("suppression_state").map_err(db)?,
                            "suppression state",
                        )?,
                        created_at: row.try_get("created_at").map_err(db)?,
                    },
                    revision: EpisodeRevision {
                        episode_revision_id: EpisodeRevisionId(revision_id),
                        episode_id: EpisodeId(row.try_get("episode_id").map_err(db)?),
                        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
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
                            row.try_get("experience_time_kind").map_err(db)?,
                            row.try_get("experience_time_start").map_err(db)?,
                            row.try_get("experience_time_end").map_err(db)?,
                        )?,
                        boundary_explanation: row.try_get("boundary_explanation").map_err(db)?,
                        formed_at: row.try_get("formed_at").map_err(db)?,
                        recorded_at: row.try_get("recorded_at").map_err(db)?,
                        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
                    },
                    members: members.remove(&revision_id).unwrap_or_default(),
                    basis: basis.remove(&revision_id).unwrap_or_default(),
                    relations: relations.remove(&revision_id).unwrap_or_default(),
                })
            })
            .map(|result: Result<EpisodeView>| {
                result.map(|view| (view.revision.episode_revision_id.0, view))
            })
            .collect()
    }

    pub(in crate::service) async fn episode_supports_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        revisions: &[Uuid],
    ) -> Result<(
        std::collections::HashMap<Uuid, Vec<RevisionBasis>>,
        std::collections::HashMap<Uuid, Vec<EpisodeMember>>,
    )> {
        use std::collections::HashMap;
        let mut members: HashMap<Uuid, Vec<EpisodeMember>> = HashMap::new();
        for row in sqlx::query("SELECT episode_revision_id,ordinal,ref_kind,ref_value,role FROM episode_revision_members WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,ordinal").bind(revisions).fetch_all(&mut **tx).await.map_err(db)? {
            members.entry(row.try_get("episode_revision_id").map_err(db)?).or_default().push(EpisodeMember {
                ordinal: row.try_get("ordinal").map_err(db)?,
                reference: parse_reference(&row.try_get::<String,_>("ref_kind").map_err(db)?, &row.try_get::<String,_>("ref_value").map_err(db)?)?,
                role: row.try_get("role").map_err(db)?,
            });
        }
        let mut basis: HashMap<Uuid, Vec<RevisionBasis>> = HashMap::new();
        for row in sqlx::query("SELECT episode_revision_id,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM episode_revision_basis WHERE episode_revision_id=ANY($1) ORDER BY episode_revision_id,basis_no").bind(revisions).fetch_all(&mut **tx).await.map_err(db)? {
            let revision_id: Uuid = row.try_get("episode_revision_id").map_err(db)?;
            let value = basis_from_row(&row)?;
            basis.entry(revision_id).or_default().push(value);
        }
        Ok((basis, members))
    }

    pub async fn episode_revision(
        &self,
        subject: SubjectId,
        revision: EpisodeRevisionId,
    ) -> Result<EpisodeView> {
        self.episode_views(subject, &[revision.0])
            .await?
            .remove(&revision.0)
            .ok_or_else(|| Error::NotFound("Episode revision not found".into()))
    }

    pub async fn list_episodes(
        &self,
        subject: SubjectId,
        status: &str,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<EpisodeView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        self.store.require_subject(subject).await?;
        if !matches!(status, "" | "accepted" | "withdrawn") {
            return Err(Error::Invalid("invalid Episode acceptance status".into()));
        }
        let mut tx = self.store.read_view().await?;
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT current_revision_id FROM episode_objects WHERE subject_id=$1 AND ($2='' OR acceptance_state=$2) AND ($3::uuid IS NULL OR episode_id>$3) ORDER BY episode_id LIMIT $4")
            .bind(subject.0).bind(status).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self.episode_views_in(&mut tx, subject, &ids, &[]).await?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("Episode page changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(db)?;
        Ok((items, more))
    }

    pub async fn episode_history(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<EpisodeView>, bool)> {
        if !(1..=200).contains(&limit) {
            return Err(Error::Invalid("page limit is out of bounds".into()));
        }
        self.store
            .validate_reference(subject, &CognitiveRef::Episode(episode))
            .await?;
        let mut tx = self.store.read_view().await?;
        let after_no = if let Some(after) = after {
            Some(sqlx::query_scalar::<_, i32>("SELECT revision_no FROM episode_revisions WHERE subject_id=$1 AND episode_id=$2 AND episode_revision_id=$3")
                .bind(subject.0).bind(episode.0).bind(after).fetch_optional(&mut *tx).await.map_err(db)?
                .ok_or_else(|| Error::Invalid("invalid Episode history cursor".into()))?)
        } else {
            None
        };
        let mut ids: Vec<Uuid> = sqlx::query_scalar("SELECT episode_revision_id FROM episode_revisions WHERE subject_id=$1 AND episode_id=$2 AND ($3::int IS NULL OR revision_no>$3) ORDER BY revision_no LIMIT $4")
            .bind(subject.0).bind(episode.0).bind(after_no).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?;
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self.episode_views_in(&mut tx, subject, &ids, &[]).await?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("Episode history changed while reading".into()))
            })
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(db)?;
        Ok((items, more))
    }
}
