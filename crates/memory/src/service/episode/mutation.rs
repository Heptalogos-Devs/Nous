// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn create_episode(&self, input: EpisodeInput) -> Result<EpisodeView> {
        let started_at = self.cognition.now(input.subject);
        validate_episode_input(
            &input.track_key,
            &input.title,
            &input.experience_time,
            &input.boundary_explanation,
            &input.members,
            &input.basis,
        )?;
        self.store.require_subject(input.subject).await?;
        let digest = operation_digest(
            "create_episode",
            input.subject,
            &serde_json::json!({
                "track_key": input.track_key,
                "title": input.title,
                "parent": input.parent_episode_revision_id,
                "experience_time": input.experience_time,
                "boundary_explanation": input.boundary_explanation,
                "producer_signature_id": input.producer_signature_id,
                "members": input.members,
                "basis": input.basis,
            }),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Memory,
                input.subject,
                input.operation_id,
                "create_episode",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    let result = receipt.results.first().ok_or_else(|| {
                        Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InternalFailure,
                            "Episode receipt has no result",
                        )
                    })?;
                    let CognitiveRef::Episode(id) = &result.reference else {
                        return Err(Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InternalFailure,
                            "Episode receipt has an invalid result reference",
                        ));
                    };
                    let revision = receipt_revision_id(&receipt, &CognitiveRef::Episode(*id))?;
                    return self
                        .episode(input.subject, *id, Some(EpisodeRevisionId(revision)))
                        .await;
                }
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::Unavailable,
                    "create Episode operation is already in progress",
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        validate_episode_refs_in_tx(&self.store, mutation.tx(), input.subject, &input.members)
            .await?;
        validate_episode_basis_in_tx(&self.store, mutation.tx(), input.subject, &input.basis)
            .await?;
        validate_parent_and_overlap(
            mutation.tx(),
            input.subject,
            &input.track_key,
            input.parent_episode_revision_id,
            &input.experience_time,
            None,
        )
        .await?;
        let episode_id = EpisodeId::new();
        let revision_id = EpisodeRevisionId::new();
        let formed_at = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started_at)
            .await?;
        let now = self.cognition.now(input.subject);
        let (time_kind, time_start, time_end) = temporal_columns(&input.experience_time);
        sqlx::query("INSERT INTO cognition_objects(object_kind,object_id,subject_id,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,created_at,track_key) VALUES('episode',$1,$2,$3,1,'accepted','valid','normal','normal',$4,$5)")
            .bind(episode_id.0).bind(input.subject.0).bind(revision_id.0).bind(now).bind(&input.track_key)
            .execute(&mut **mutation.tx()).await.map_err(db)?;
        insert_episode_revision_with_intent(
            &self.store,
            mutation.tx(),
            &input,
            episode_id,
            revision_id,
            None,
            None,
            1,
            time_kind,
            time_start,
            time_end,
            formed_at,
            now,
        )
        .await?;

        let authority_seq = mutation.invalidate(ProjectionInvalidation::text()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::EpisodeRevision(revision_id),
            authority_seq,
        )
        .await?;
        self.schedule_episode_in(
            mutation.tx(),
            input.subject,
            revision_id,
            &input.track_key,
            authority_seq,
            true,
        )
        .await?;
        mutation
            .commit(&[MutationResult::revised(
                CognitiveRef::Episode(episode_id),
                revision_id.0,
                Some(1),
            )])
            .await?;
        self.episode(input.subject, episode_id, None).await
    }

    pub(in crate::service) async fn schedule_episode_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        revision: EpisodeRevisionId,
        track: &str,
        authority_seq: i64,
        semantic_review: bool,
    ) -> Result<()> {
        let delay = self
            .configuration
            .snapshot_for_subject(subject)?
            .get(nous_runtime::SETTLE_DELAY_KEY)?;
        let now = self.cognition.now(subject);
        let due_at = now + chrono::Duration::seconds(delay as i64);
        self.cognition
            .wake_blocked_maintenance_in(
                tx,
                &nous_runtime::MaintenanceRequest {
                    subject,
                    kind: "episode_resegment".into(),
                    scope_kind: "track".into(),
                    scope_ref: track.to_owned(),
                    trigger_authority_seq: authority_seq,
                    due_at,
                    priority: 60,
                },
            )
            .await?;
        let consolidation_due = now
            + chrono::Duration::seconds(
                self.configuration
                    .snapshot_for_subject(subject)?
                    .get(super::longitudinal_policy::CONSOLIDATION_DELAY)? as i64,
            );
        for (kind, scope_kind, scope_ref) in [
            (
                "episode_resegment",
                "episode_revision",
                revision.0.to_string(),
            ),
            ("journal_review", "track", track.to_owned()),
            (
                "memory_consolidate",
                "episode_revision",
                revision.0.to_string(),
            ),
        ] {
            if kind == "episode_resegment" && !semantic_review {
                continue;
            }
            self.cognition
                .enqueue_maintenance_in(
                    tx,
                    &nous_runtime::MaintenanceRequest {
                        subject,
                        kind: kind.into(),
                        scope_kind: scope_kind.into(),
                        scope_ref,
                        trigger_authority_seq: authority_seq,
                        due_at: if kind == "memory_consolidate" {
                            consolidation_due
                        } else {
                            due_at
                        },
                        priority: 30,
                    },
                )
                .await?;
        }
        Ok(())
    }

    pub async fn revise_episode(&self, input: ReviseEpisodeInput) -> Result<EpisodeView> {
        let started_at = self.cognition.now(input.subject);
        validate_episode_input(
            "",
            &input.title,
            &input.experience_time,
            &input.boundary_explanation,
            &input.members,
            &input.basis,
        )?;
        if !matches!(input.intent.as_str(), "resegment" | "reinterpret") {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid Episode revision intent",
            ));
        }
        let digest = operation_digest(
            "revise_episode",
            input.subject,
            &serde_json::json!({
                "episode_id": input.episode_id,
                "expected_object_epoch": input.expected_object_epoch,
                "intent": input.intent,
                "title": input.title,
                "parent": input.parent_episode_revision_id,
                "experience_time": input.experience_time,
                "boundary_explanation": input.boundary_explanation,
                "members": input.members,
                "basis": input.basis,
            }),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Memory,
                input.subject,
                input.operation_id,
                "revise_episode",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    let revision =
                        receipt_revision_id(&receipt, &CognitiveRef::Episode(input.episode_id))?;
                    return self
                        .episode(
                            input.subject,
                            input.episode_id,
                            Some(EpisodeRevisionId(revision)),
                        )
                        .await;
                }
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::Unavailable,
                    "revise Episode operation is already in progress",
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query("SELECT track_key,current_revision_id,object_epoch,purge_state FROM cognition_objects WHERE subject_id=$1 AND object_kind='episode' AND object_id=$2 FOR UPDATE")
            .bind(input.subject.0).bind(input.episode_id.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?
            .ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::NotFound, "Episode not found"))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        crate::service::state::fence_epoch(ErrorOwner::Memory, epoch, input.expected_object_epoch)?;
        if row.try_get::<String, _>("purge_state").map_err(db)? == "purging" {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::FailedPrecondition,
                "Episode is purging",
            ));
        }
        validate_episode_refs_in_tx(&self.store, mutation.tx(), input.subject, &input.members)
            .await?;
        validate_episode_basis_in_tx(&self.store, mutation.tx(), input.subject, &input.basis)
            .await?;
        let parent = input.parent_episode_revision_id;
        validate_parent_and_overlap(
            mutation.tx(),
            input.subject,
            &row.try_get::<String, _>("track_key").map_err(db)?,
            parent,
            &input.experience_time,
            Some(input.episode_id.0),
        )
        .await?;
        if let Some(parent) = parent {
            ensure_no_parent_cycle(mutation.tx(), input.episode_id.0, parent.0).await?;
        }
        let current: Uuid = row.try_get("current_revision_id").map_err(db)?;
        let revision_no: i32 = sqlx::query_scalar(
            "SELECT revision_no+1 FROM cognition_revision_headers WHERE object_kind='episode' AND revision_id=$1",
        )
        .bind(current)
        .fetch_one(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let revision_id = EpisodeRevisionId::new();
        let formed_at = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started_at)
            .await?;
        let now = self.cognition.now(input.subject);
        let (kind, start, end) = temporal_columns(&input.experience_time);
        let create = EpisodeInput {
            operation_id: input.operation_id,
            subject: input.subject,
            track_key: row.try_get("track_key").map_err(db)?,
            title: input.title,
            parent_episode_revision_id: input.parent_episode_revision_id,
            experience_time: input.experience_time,
            boundary_explanation: input.boundary_explanation,

            producer_signature_id: input.producer_signature_id,
            members: input.members,
            basis: input.basis,
        };
        insert_episode_revision_with_intent(
            &self.store,
            mutation.tx(),
            &create,
            input.episode_id,
            revision_id,
            Some(EpisodeRevisionId(current)),
            Some(input.intent),
            revision_no,
            kind,
            start,
            end,
            formed_at,
            now,
        )
        .await?;
        sqlx::query("UPDATE cognition_objects SET current_revision_id=$3,object_epoch=object_epoch+1,integrity_state='valid' WHERE subject_id=$1 AND object_kind='episode' AND object_id=$2")
            .bind(input.subject.0).bind(input.episode_id.0).bind(revision_id.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::EpisodeRevision(revision_id),
            sequence,
        )
        .await?;
        self.schedule_episode_in(
            mutation.tx(),
            input.subject,
            revision_id,
            &create.track_key,
            sequence,
            true,
        )
        .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            input.subject,
            "episode",
            &[input.episode_id.0],
            sequence,
            "source_revised",
        )
        .await?;

        mutation
            .commit(&[MutationResult::revised(
                CognitiveRef::Episode(input.episode_id),
                revision_id.0,
                Some(epoch + 1),
            )])
            .await?;
        self.episode(input.subject, input.episode_id, None).await
    }

    pub async fn link_episode_revisions(
        &self,
        subject: SubjectId,
        operation_id: OperationId,
        from: EpisodeRevisionId,
        to: EpisodeRevisionId,
        relation: String,
    ) -> Result<()> {
        if from == to
            || !matches!(
                relation.as_str(),
                "split_from" | "merged_from" | "temporal_successor" | "derived_from"
            )
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid Episode relation",
            ));
        }
        let digest = operation_digest(
            "link_episode_revisions",
            subject,
            &serde_json::json!({"from":from,"to":to,"relation":relation}),
        )?;
        let mut mutation = match self
            .start_mutation(
                ErrorOwner::Memory,
                subject,
                operation_id,
                "link_episode_revisions",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return Ok(());
                }
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::Unavailable,
                    "Episode relation operation is already in progress",
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        for revision in [from, to] {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind='episode' AND revision_id=$2)").bind(subject.0).bind(revision.0).fetch_one(&mut **mutation.tx()).await.map_err(db)?;
            if !exists {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "Episode revision not found",
                ));
            }
        }
        sqlx::query("INSERT INTO episode_revision_relations(from_revision_id,to_revision_id,relation,created_at) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(from.0).bind(to.0).bind(&relation).bind(self.cognition.now(subject)).execute(&mut **mutation.tx()).await.map_err(db)?;

        mutation.commit(&[]).await
    }
}
