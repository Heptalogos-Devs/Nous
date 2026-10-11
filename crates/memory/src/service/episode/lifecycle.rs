// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn suppress_episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
    ) -> Result<EpisodeView> {
        self.mutate_episode_lifecycle(
            subject,
            episode,
            operation_id,
            expected,
            "suppression_state",
            "normal",
            "suppressed",
        )
        .await
    }

    pub async fn restore_episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
    ) -> Result<EpisodeView> {
        self.mutate_episode_lifecycle(
            subject,
            episode,
            operation_id,
            expected,
            "suppression_state",
            "suppressed",
            "normal",
        )
        .await
    }

    pub async fn withdraw_episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
    ) -> Result<EpisodeView> {
        self.mutate_episode_lifecycle(
            subject,
            episode,
            operation_id,
            expected,
            "acceptance_state",
            "accepted",
            "withdrawn",
        )
        .await
    }

    pub async fn reaccept_episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
    ) -> Result<EpisodeView> {
        self.mutate_episode_lifecycle(
            subject,
            episode,
            operation_id,
            expected,
            "acceptance_state",
            "withdrawn",
            "accepted",
        )
        .await
    }

    pub(in crate::service) async fn mutate_episode_lifecycle(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
        field: &str,
        from: &str,
        to: &str,
    ) -> Result<EpisodeView> {
        let digest = operation_digest(
            "episode_lifecycle",
            subject,
            &serde_json::json!({"episode":episode,"expected":expected,"field":field,"from":from,"to":to}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation_id, "episode_lifecycle", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self.episode(subject, episode, None).await;
                }
                return Err(Error::Unavailable(
                    "Episode lifecycle operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT object_epoch,suppression_state,acceptance_state,purge_state FROM episode_objects WHERE subject_id=$1 AND episode_id=$2 FOR UPDATE")
            .bind(subject.0).bind(episode.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("Episode not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        crate::service::state::fence_epoch(epoch, expected)?;
        if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            return Err(Error::FailedPrecondition("Episode is purging".into()));
        }
        let current: String = row.try_get(field).map_err(db)?;
        require_transition(&current, from)?;
        if field == "acceptance_state" && to == "accepted" {
            validate_episode_reaccept_in(mutation.tx(), subject, episode).await?;
        }
        let query = match field {
            "suppression_state" => sqlx::query(
                "UPDATE episode_objects SET suppression_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND episode_id=$2",
            ),
            "acceptance_state" => sqlx::query(
                "UPDATE episode_objects SET acceptance_state=$3,object_epoch=object_epoch+1 WHERE subject_id=$1 AND episode_id=$2",
            ),
            _ => {
                return Err(Error::Internal(
                    "unsupported Episode lifecycle field".into(),
                ));
            }
        };
        query
            .bind(subject.0)
            .bind(episode.0)
            .bind(to)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "episode",
            &[episode.0],
            sequence,
            "source_lifecycle_changed",
        )
        .await?;

        mutation
            .commit(
                "episode",
                Some(&episode.0.to_string()),
                None,
                Some(epoch + 1),
            )
            .await?;
        self.episode(subject, episode, None).await
    }

    pub async fn purge_episode(
        &self,
        subject: SubjectId,
        episode: EpisodeId,
        operation_id: OperationId,
        expected: i64,
    ) -> Result<()> {
        let digest = operation_digest(
            "purge_episode",
            subject,
            &serde_json::json!({"episode":episode,"expected":expected}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation_id, "purge_episode", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return Ok(());
                }
                return Err(Error::Unavailable(
                    "Episode purge operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let epoch: i64 = sqlx::query_scalar("SELECT object_epoch FROM episode_objects WHERE subject_id=$1 AND episode_id=$2 FOR UPDATE").bind(subject.0).bind(episode.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(|| Error::NotFound("Episode not found".into()))?;
        crate::service::state::fence_epoch(epoch, expected)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "episode",
            &[episode.0],
            sequence,
            "source_purged",
        )
        .await?;
        self.purge_episode_runtime_refs_in(mutation.tx(), subject, episode)
            .await?;
        sqlx::query("DELETE FROM episode_objects WHERE subject_id=$1 AND episode_id=$2")
            .bind(subject.0)
            .bind(episode.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;

        mutation
            .commit("purged", Some(&episode.0.to_string()), None, None)
            .await
    }

    pub(in crate::service) async fn purge_episode_runtime_refs_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        episode: EpisodeId,
    ) -> Result<()> {
        let refs: Vec<String> = sqlx::query_scalar("SELECT episode_revision_id::text FROM episode_revisions WHERE subject_id=$1 AND episode_id=$2")
            .bind(subject.0).bind(episode.0).fetch_all(&mut **tx).await.map_err(db)?;
        let mut workflow_refs = refs.clone();
        workflow_refs.push(episode.0.to_string());
        self.purge_workflow_content_in(tx, subject, &workflow_refs)
            .await?;
        sqlx::query("INSERT INTO purged_use_receipts(subject_id,consumer_ref,event_id,request_digest,purged_at) SELECT subject_id,consumer_ref,event_id,request_digest,$3 FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='episode_revision' AND ref_value=ANY($2::text[]) ON CONFLICT DO NOTHING")
            .bind(subject.0).bind(&refs).bind(self.cognition.now(subject)).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("DELETE FROM cognitive_use_events WHERE subject_id=$1 AND ref_kind='episode_revision' AND ref_value=ANY($2::text[])")
            .bind(subject.0).bind(&refs).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("DELETE FROM resident_refs WHERE ref_kind='episode_revision' AND ref_value=ANY($1::text[])")
            .bind(&refs).execute(&mut **tx).await.map_err(db)?;
        sqlx::query("DELETE FROM work_context_refs WHERE ref_kind='episode_revision' AND ref_value=ANY($1::text[])")
            .bind(&refs).execute(&mut **tx).await.map_err(db)?;

        Ok(())
    }
}
