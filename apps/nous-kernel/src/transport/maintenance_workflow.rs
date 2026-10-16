// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[tonic::async_trait]
impl k::kernel_maintenance_service_server::KernelMaintenanceService for KernelService {
    async fn reflection_memory_candidates(
        &self,
        r: Request<k::ReflectionMemoryCandidatesRequest>,
    ) -> std::result::Result<Response<k::ReflectionMemoryCandidatesResponse>, Status> {
        rpc_reply(async {
            let r = r.into_inner();
            let refs = r
                .references
                .into_iter()
                .map(from_ref)
                .collect::<nous_core::Result<Vec<_>>>()?;
            let (items, omitted) = self
                .0
                .require_memory()?
                .reflection_candidates(
                    SubjectId(id(&r.subject_id)?),
                    &refs,
                    r.max_items as usize,
                    r.max_bytes as usize,
                )
                .await?;
            Ok(k::ReflectionMemoryCandidatesResponse {
                items: items.into_iter().map(super::memory::view).collect(),
                omitted,
            })
        })
        .await
    }

    async fn prepare_subject_reflection(
        &self,
        r: Request<k::PrepareSubjectReflectionRequest>,
    ) -> std::result::Result<Response<k::SubjectReflectionBatch>, Status> {
        rpc_reply(async {
            let selected =
                super::longitudinal::need(required(r.into_inner().selected, "selected")?)?;
            let b = self.0.cognition.prepare_reflection(&selected).await?;
            Ok(k::SubjectReflectionBatch {
                selected: Some(super::longitudinal::need_proto(b.need)),
                from_sequence: b.from_seq,
                through_sequence: b.through_seq,
                sources: b
                    .sources
                    .into_iter()
                    .map(|s| k::SubjectReflectionSource {
                        reference: Some(to_ref(s.reference)),
                        recorded_sequence: s.recorded_seq,
                        work_context_id: s.work_context.map(|s| s.0.to_string()),
                        work_context_revision: s.work_context_revision,
                        session_id: s.session.map(|s| s.0.to_string()),
                    })
                    .collect(),
                more: b.more,
                domains: b.domains,
                source_max_bytes: b.source_bytes,
                candidate_limit: b.candidate_limit,
                candidate_max_bytes: b.candidate_bytes,
                max_actions: b.max_actions,
                through_change_id: b.through_change,
                changes: b
                    .changes
                    .into_iter()
                    .map(|c| k::SubjectReflectionSourceChange {
                        change_id: c.change_id,
                        reference: Some(to_ref(c.reference)),
                        reason: c.reason,
                        recorded_at: Some(timestamp(c.recorded_at)),
                    })
                    .collect(),
            })
        })
        .await
    }
    async fn finish_subject_reflection(
        &self,
        r: Request<k::FinishSubjectReflectionRequest>,
    ) -> std::result::Result<Response<k::FinishSubjectReflectionResponse>, Status> {
        rpc_reply(async {
            let r = r.into_inner();
            let selected = super::longitudinal::need(required(r.selected, "selected")?)?;
            self.0
                .cognition
                .finish_reflection(
                    &selected,
                    r.through_sequence,
                    r.through_change_id,
                    &r.operation_id,
                    &r.status,
                    r.problem_code.as_deref(),
                )
                .await?;
            Ok(k::FinishSubjectReflectionResponse {
                operations_consumed: if r.status == "blocked" { 0 } else { 1 },
            })
        })
        .await
    }
    async fn inspect_subject_reflection(
        &self,
        r: Request<p::ReflectionInspectRequest>,
    ) -> std::result::Result<Response<p::ReflectionState>, Status> {
        rpc_reply(async {
            let r = r.into_inner();
            let rows = self
                .0
                .store
                .reflection_status(
                    SubjectId(id(&r.subject_id)?),
                    r.operation_id.as_deref(),
                    if r.limit == 0 { 16 } else { r.limit as usize },
                    if r.max_payload_bytes == 0 {
                        1024 * 1024
                    } else {
                        r.max_payload_bytes as usize
                    },
                )
                .await?;
            Ok(p::ReflectionState {
                through_sequence: rows.through,
                last_operation_id: rows.last_operation,
                last_status: rows.last_status,
                last_problem_code: rows.last_problem,
                pending: rows
                    .pending
                    .into_iter()
                    .map(|(need, rev, seq, state, problem)| p::ReflectionPending {
                        need_id: need.to_string(),
                        trigger_revision: rev as u64,
                        trigger_sequence: seq,
                        state,
                        problem_code: problem,
                    })
                    .collect(),
                recent: rows
                    .recent
                    .into_iter()
                    .map(|r| p::ReflectionRecord {
                        operation_id: r.operation,
                        semantic_digest: r.digest,
                        proposal_json: r.proposal,
                        outcome_json: r.outcome,
                        body_omitted: r.omitted,
                        recorded_at: Some(timestamp(r.recorded_at)),
                    })
                    .collect(),
                more: rows.more,
            })
        })
        .await
    }

    async fn refresh_maintenance(
        &self,
        request: Request<k::RefreshMaintenanceRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        rpc_reply(KernelService::refresh_maintenance(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn plan_maintenance(
        &self,
        request: Request<k::PlanMaintenanceRequest>,
    ) -> std::result::Result<Response<k::MaintenancePlan>, Status> {
        rpc_reply(KernelService::plan_maintenance(self, request.into_inner())).await
    }
    async fn get_maintenance_policy(
        &self,
        request: Request<p::SubjectRequest>,
    ) -> std::result::Result<Response<k::MaintenancePolicy>, Status> {
        rpc_reply(KernelService::get_maintenance_policy(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn select_maintenance(
        &self,
        request: Request<k::SelectMaintenanceRequest>,
    ) -> std::result::Result<Response<k::SelectMaintenanceResponse>, Status> {
        rpc_reply(KernelService::select_maintenance(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn finish_maintenance(
        &self,
        request: Request<k::FinishMaintenanceRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        rpc_reply(KernelService::finish_maintenance(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn organize_experience(
        &self,
        request: Request<k::OrganizeExperienceRequest>,
    ) -> std::result::Result<Response<k::OrganizeExperienceResponse>, Status> {
        rpc_reply(KernelService::organize_experience(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn apply_episode_partition(
        &self,
        request: Request<k::ApplyEpisodePartitionRequest>,
    ) -> std::result::Result<Response<k::ApplyEpisodePartitionResponse>, Status> {
        rpc_reply(KernelService::apply_episode_partition(
            self,
            request.into_inner(),
        ))
        .await
    }
    async fn commit_journal(
        &self,
        request: Request<k::CommitJournalRequest>,
    ) -> std::result::Result<Response<p::JournalResponse>, Status> {
        rpc_reply(KernelService::commit_journal(self, request.into_inner())).await
    }
}
