// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EpisodeRevisionId, JournalId, JournalRevisionId, OperationId, Result};
use nous_memory::{
    EpisodePartitionInput, EpisodePartitionSegment, EpisodePartitionSource, JournalInput,
    JournalPoint, JournalTarget,
};
use nous_runtime::{MaintenanceDisposition, MaintenanceNeed};

fn need_proto(value: MaintenanceNeed) -> k::MaintenanceNeed {
    k::MaintenanceNeed {
        need_id: value.need_id.to_string(),
        subject_id: value.subject_id.0.to_string(),
        kind: value.kind,
        scope_kind: value.scope_kind,
        scope_ref: value.scope_ref,
        trigger_authority_seq: value.trigger_authority_seq,
        trigger_revision: value.trigger_revision,
        due_at: Some(timestamp(value.due_at)),
        priority: value.priority,
        state: value.state,
        lease_token: value.lease_token.map(|id| id.to_string()),
        lease_until: value.lease_until.map(timestamp),
        attempt_count: value.attempt_count,
        retry_count: value.retry_count,
        last_problem_code: value.last_problem_code,
        created_at: Some(timestamp(value.created_at)),
        updated_at: Some(timestamp(value.updated_at)),
    }
}
pub(super) fn need(value: k::MaintenanceNeed) -> Result<MaintenanceNeed> {
    Ok(MaintenanceNeed {
        need_id: id(&value.need_id)?,
        subject_id: SubjectId(id(&value.subject_id)?),
        kind: value.kind,
        scope_kind: value.scope_kind,
        scope_ref: value.scope_ref,
        trigger_authority_seq: value.trigger_authority_seq,
        trigger_revision: value.trigger_revision,
        due_at: required(time(value.due_at)?, "due_at")?,
        priority: value.priority,
        state: value.state,
        lease_token: value.lease_token.as_deref().map(id).transpose()?,
        lease_until: time(value.lease_until)?,
        attempt_count: value.attempt_count,
        retry_count: value.retry_count,
        last_problem_code: value.last_problem_code,
        created_at: required(time(value.created_at)?, "created_at")?,
        updated_at: required(time(value.updated_at)?, "updated_at")?,
    })
}
fn source(value: k::EpisodePartitionSource) -> Result<EpisodePartitionSource> {
    Ok(EpisodePartitionSource {
        revision: EpisodeRevisionId(id(&value.revision_id)?),
        expected_epoch: value.expected_epoch,
    })
}

impl KernelService {
    pub(super) async fn refresh_maintenance(
        &self,
        input: k::RefreshMaintenanceRequest,
    ) -> Result<()> {
        let claimed = need(required(input.claimed, "claimed")?)?;
        self.0
            .cognition
            .enqueue_maintenance(nous_runtime::MaintenanceRequest {
                subject: claimed.subject_id,
                kind: claimed.kind,
                scope_kind: claimed.scope_kind,
                scope_ref: claimed.scope_ref,
                trigger_authority_seq: self.0.store.authority_seq(claimed.subject_id).await?,
                due_at: self.0.cognition.now(claimed.subject_id),
                priority: claimed.priority,
            })
            .await?;
        Ok(())
    }
    pub(super) async fn get_maintenance_policy(
        &self,
        input: p::SubjectRequest,
    ) -> Result<k::MaintenancePolicy> {
        let subject = if input.subject_id.is_empty() {
            None
        } else {
            Some(SubjectId(id(&input.subject_id)?))
        };
        let snapshot = if let Some(subject) = subject {
            self.0.store.require_subject(subject).await?;
            self.0.configuration.snapshot_for_subject(subject)?
        } else {
            self.0.configuration.active_system_snapshot()?
        };
        Ok(k::MaintenancePolicy {
            enabled: snapshot.get(nous_runtime::MAINTENANCE_ENABLED)?,
            poll_interval_seconds: snapshot.get(nous_runtime::POLL_INTERVAL)? as u32,
            max_operations: snapshot.get(nous_runtime::MAX_OPERATIONS)? as u32,
            worker_lease_seconds: snapshot.get(nous_runtime::WORKER_LEASE)? as u32,
            retry_initial_seconds: snapshot.get(nous_runtime::RETRY_INITIAL)? as u32,
            retry_max_seconds: snapshot.get(nous_runtime::RETRY_MAX)? as u32,
            retry_max_attempts: snapshot.get(nous_runtime::RETRY_ATTEMPTS)? as u32,
            max_model_calls: snapshot.get(nous_runtime::MAX_MODEL_CALLS)? as u32,
            max_elapsed_ms: snapshot.get(nous_runtime::MAX_ELAPSED)? as u32,
            experience_batch_size: snapshot.get(nous_runtime::EXPERIENCE_BATCH_SIZE)? as u32,
            cognitive_now: subject.map(|subject| timestamp(self.0.cognition.now(subject))),
        })
    }

    pub(super) async fn claim_maintenance(
        &self,
        input: k::ClaimMaintenanceRequest,
    ) -> Result<k::ClaimMaintenanceResponse> {
        Ok(k::ClaimMaintenanceResponse {
            needs: self
                .0
                .cognition
                .lease_maintenance(
                    SubjectId(id(&input.subject_id)?),
                    &input.allowed_kinds,
                    input.limit,
                    input.lease_seconds,
                    input.model_execution_digest.as_deref(),
                )
                .await?
                .into_iter()
                .map(need_proto)
                .collect(),
        })
    }
    pub(super) async fn finish_maintenance(
        &self,
        input: k::FinishMaintenanceRequest,
    ) -> Result<()> {
        let claimed = need(required(input.claimed, "claimed")?)?;
        let disposition = match input.disposition.as_str() {
            "satisfied" => MaintenanceDisposition::Satisfied,
            "obsolete" => MaintenanceDisposition::Obsolete,
            "pending" => MaintenanceDisposition::Pending {
                due_at: required(time(input.next_due)?, "next_due")?,
                problem_code: input.problem_code,
            },
            "blocked" => MaintenanceDisposition::Blocked {
                problem_code: required(input.problem_code, "problem_code")?,
            },
            "retry" => MaintenanceDisposition::Retry {
                delay_seconds: input.retry_delay_seconds,
                problem_code: required(input.problem_code, "problem_code")?,
            },
            _ => return Err(Error::Invalid("invalid maintenance disposition".into())),
        };
        self.0
            .cognition
            .acknowledge_maintenance(&claimed, disposition)
            .await
    }
    pub(super) async fn organize_experience(
        &self,
        input: k::OrganizeExperienceRequest,
    ) -> Result<k::OrganizeExperienceResponse> {
        let result = self
            .0
            .organize_experience(SubjectId(id(&input.subject_id)?), input.limit, input.close)
            .await?;
        Ok(k::OrganizeExperienceResponse {
            processed_count: result.processed_count as u32,
            episodes: result.episodes.into_iter().map(episode::view).collect(),
            next_due: result.next_due.map(timestamp),
        })
    }
    pub(super) async fn apply_episode_partition(
        &self,
        input: k::ApplyEpisodePartitionRequest,
    ) -> Result<k::ApplyEpisodePartitionResponse> {
        let result = self
            .require_memory()?
            .apply_episode_partition(EpisodePartitionInput {
                operation_id: OperationId(id(&input.operation_id)?),
                subject: SubjectId(id(&input.subject_id)?),
                expected_authority_seq: input.expected_authority_seq,
                sources: input
                    .sources
                    .into_iter()
                    .map(source)
                    .collect::<Result<_>>()?,
                ordered_occurrences: input
                    .ordered_occurrences
                    .iter()
                    .map(|value| id(value).map(nous_core::OccurrenceId))
                    .collect::<Result<_>>()?,
                segments: input
                    .segments
                    .into_iter()
                    .map(|segment| EpisodePartitionSegment {
                        member_indices: segment
                            .member_indices
                            .into_iter()
                            .map(|index| index as usize)
                            .collect(),
                        title: segment.title,
                        boundary_explanation: segment.boundary_explanation,
                    })
                    .collect(),
                producer: input.producer.map(from_producer).transpose()?,
            })
            .await?;
        Ok(k::ApplyEpisodePartitionResponse {
            episodes: result.into_iter().map(episode::view).collect(),
        })
    }
    pub(super) async fn commit_journal(
        &self,
        input: k::CommitJournalRequest,
    ) -> Result<p::JournalResponse> {
        let target = input
            .target
            .map(|target| -> Result<JournalTarget> {
                Ok(JournalTarget {
                    journal_id: JournalId(id(&target.journal_id)?),
                    expected_revision: JournalRevisionId(id(&target.expected_revision_id)?),
                    expected_epoch: target.expected_epoch,
                    intent: target.intent,
                })
            })
            .transpose()?;
        let points = input
            .points
            .into_iter()
            .enumerate()
            .map(|(ordinal, point)| {
                if point.ordinal != ordinal as i32 {
                    return Err(Error::Invalid(
                        "Journal point ordinals must be contiguous".into(),
                    ));
                }
                Ok(JournalPoint {
                    role: enum_value(&point.role)?,
                    text: point.text,
                    basis: point.basis.into_iter().map(basis).collect::<Result<_>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let result = self
            .require_memory()?
            .commit_journal(JournalInput {
                operation_id: OperationId(id(&input.operation_id)?),
                subject: SubjectId(id(&input.subject_id)?),
                expected_authority_seq: input.expected_authority_seq,
                target,
                sources: input
                    .sources
                    .into_iter()
                    .map(source)
                    .collect::<Result<_>>()?,
                title: input.title,
                narrative: input.narrative,
                points,
                producer: input.producer.map(from_producer).transpose()?,
            })
            .await?;
        Ok(journal::journal_response(result))
    }
}
