// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::model::private_workflow_owner;
use super::workflow_convert as envelope;
use super::*;
use nous_core::Result;
#[tonic::async_trait]
impl k::kernel_model_workflow_service_server::KernelModelWorkflowService for KernelService {
    async fn find_workflow(
        &self,
        request: Request<k::FindWorkflowRequest>,
    ) -> std::result::Result<Response<k::FoundWorkflow>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            private_workflow_owner(&input.owner)?;
            let row = self
                .0
                .store
                .find_model_workflow(
                    SubjectId(id(&input.subject_id)?),
                    &input.owner,
                    &input.operation_key,
                    &input.semantic_digest,
                )
                .await?;
            use nous_persistence::WorkflowRecord;
            Ok(k::FoundWorkflow {
                state: row.map(|row| match row {
                    WorkflowRecord::Pending(value) => {
                        k::found_workflow::State::Pending(envelope::pending_proto(value))
                    }
                    WorkflowRecord::Completed(value) => {
                        k::found_workflow::State::Completed(envelope::completed_proto(value))
                    }
                }),
            })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn reserve_workflow(
        &self,
        request: Request<k::ReserveWorkflowRequest>,
    ) -> std::result::Result<Response<k::WorkflowReservation>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let subject = SubjectId(id(&input.subject_id)?);
            if input.owner == "memory" {
                id(&input.operation_key)?;
                if self.0.memory.is_none() {
                    return Err(Error::new(
                        nous_core::ErrorOwner::ModelWorkflow,
                        nous_core::ErrorCode::Unavailable,
                        "Memory owner unavailable",
                    ));
                }
            }
            private_workflow_owner(&input.owner)?;
            let mut snapshot = envelope::snapshot(required(input.snapshot, "workflow snapshot")?)?;
            if let Some(claim) = &snapshot.maintenance_claim {
                if input.owner != "memory" {
                    return Err(Error::new(
                        nous_core::ErrorOwner::ModelWorkflow,
                        nous_core::ErrorCode::InvalidRequest,
                        "Invalid request",
                    ));
                }
                let expected = uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_OID,
                    format!(
                        "{}:{}:{}",
                        claim.need_id, claim.trigger_authority_seq, claim.trigger_revision
                    )
                    .as_bytes(),
                );
                if input.operation_key != expected.to_string() {
                    return Err(Error::new(
                        nous_core::ErrorOwner::ModelWorkflow,
                        nous_core::ErrorCode::InvalidRequest,
                        "Invalid request",
                    ));
                }
            }
            if input.owner == "memory" {
                snapshot.cognitive_formed_at = Some(self.0.cognition.now(subject));
            }
            let reserved = self
                .0
                .store
                .reserve_model_workflow(
                    subject,
                    &input.owner,
                    &input.operation_key,
                    &input.semantic_digest,
                    &snapshot,
                    u64::from(input.lease_seconds),
                )
                .await?;
            use nous_persistence::WorkflowReservation;
            Ok(k::WorkflowReservation {
                state: Some(match reserved {
                    WorkflowReservation::Acquired { pending, lease } => {
                        k::workflow_reservation::State::Acquired(k::AcquiredWorkflow {
                            pending: Some(envelope::pending_proto(pending)),
                            lease: Some(envelope::lease_proto(lease)),
                        })
                    }
                    WorkflowReservation::Busy(value) => {
                        k::workflow_reservation::State::Busy(envelope::pending_proto(value))
                    }
                    WorkflowReservation::Completed(value) => {
                        k::workflow_reservation::State::Completed(envelope::completed_proto(value))
                    }
                }),
            })
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn save_workflow(
        &self,
        request: Request<k::SaveWorkflowRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let lease = envelope::lease(required(input.lease, "workflow lease")?)?;
            private_workflow_owner(lease.owner.as_str())?;
            let proposal = input.proposal.map(envelope::payload).transpose()?;
            let outcome = input.outcome.map(envelope::payload).transpose()?;
            let execution_telemetry = input
                .execution_telemetry
                .map(envelope::telemetry)
                .transpose()?;
            let operations = input
                .mutation_operations
                .iter()
                .map(|value| id(value).map(nous_core::OperationId))
                .collect::<Result<Vec<_>>>()?;
            self.0
                .store
                .save_model_workflow(
                    &lease,
                    proposal.as_ref(),
                    outcome.as_ref(),
                    execution_telemetry.as_ref(),
                    &operations,
                )
                .await
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn release_workflow(
        &self,
        request: Request<k::ReleaseWorkflowRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let input = request.into_inner();
        let lease = envelope::lease(required(input.lease, "workflow lease").map_err(status)?)
            .map_err(status)?;
        private_workflow_owner(lease.owner.as_str()).map_err(status)?;
        self.0
            .store
            .release_model_workflow(&lease)
            .await
            .map(Response::new)
            .map_err(status)
    }
}
