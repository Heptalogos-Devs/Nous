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
        let row = sqlx::query("SELECT o.episode_id,o.subject_id,o.track_key,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.created_at,r.episode_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.parent_episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end,r.boundary_explanation,r.formed_at,r.recorded_at,r.producer_signature_id FROM episode_objects o JOIN episode_revisions r ON r.episode_id=o.episode_id AND r.episode_revision_id=COALESCE($3,o.current_revision_id) WHERE o.subject_id=$1 AND o.episode_id=$2")
            .bind(subject.0).bind(episode.0).bind(revision.map(|value| value.0))
            .fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("Episode not found".into()))?;
        let revision_id: Uuid = row.try_get("episode_revision_id").map_err(db)?;
        let members = load_members(self.store.pool(), revision_id).await?;
        let basis = load_basis(self.store.pool(), revision_id).await?;
        let relations = sqlx::query("SELECT from_revision_id,to_revision_id,relation,created_at FROM episode_revision_relations WHERE from_revision_id=$1 OR to_revision_id=$1 ORDER BY created_at")
            .bind(revision_id).fetch_all(self.store.pool()).await.map_err(db)?
            .into_iter().map(|row| Ok(EpisodeRelation {
                from_revision_id: EpisodeRevisionId(row.try_get("from_revision_id").map_err(db)?),
                to_revision_id: EpisodeRevisionId(row.try_get("to_revision_id").map_err(db)?),
                relation: row.try_get("relation").map_err(db)?,
                created_at: row.try_get("created_at").map_err(db)?,
            })).collect::<Result<Vec<_>>>()?;
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
            members,
            basis,
            relations,
        })
    }

    pub async fn episode_revision(
        &self,
        subject: SubjectId,
        revision: EpisodeRevisionId,
    ) -> Result<EpisodeView> {
        let episode: Uuid = sqlx::query_scalar("SELECT episode_id FROM episode_revisions WHERE subject_id=$1 AND episode_revision_id=$2")
            .bind(subject.0).bind(revision.0).fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("Episode revision not found".into()))?;
        self.episode(subject, EpisodeId(episode), Some(revision))
            .await
    }

    pub async fn list_episodes(&self, subject: SubjectId) -> Result<Vec<EpisodeView>> {
        let ids = sqlx::query_scalar::<_, Uuid>("SELECT episode_id FROM episode_objects WHERE subject_id=$1 ORDER BY created_at,episode_id")
            .bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            result.push(self.episode(subject, EpisodeId(id), None).await?);
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
        for id in ids {
            result.push(
                self.episode(subject, episode, Some(EpisodeRevisionId(id)))
                    .await?,
            );
        }
        Ok(result)
    }
}
