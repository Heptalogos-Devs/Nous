// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::model::private_workflow_owner;
use super::workflow_convert as envelope;
use super::*;
use nous_core::Result;
#[tonic::async_trait]
impl k::kernel_model_workflow_service_server::KernelModelWorkflowService for KernelService {
    async fn open_workflow(
        &self,
        request: Request<k::OpenWorkflowRequest>,
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
            let mut snapshot = input.snapshot.map(envelope::snapshot).transpose()?;
            let maintenance_claim = input
                .maintenance_claim
                .map(envelope::maintenance_claim)
                .transpose()?;
            if let Some(claim) = &maintenance_claim {
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
            if let Some(snapshot) = &mut snapshot
                && input.owner == "memory"
            {
                snapshot.cognitive_formed_at = Some(self.0.cognition.now(subject));
            }
            let reserved = self
                .0
                .store
                .open_model_workflow(
                    subject,
                    &input.owner,
                    &input.operation_key,
                    &input.semantic_digest,
                    snapshot.as_ref(),
                    maintenance_claim.as_ref(),
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
                    WorkflowReservation::Missing => {
                        k::workflow_reservation::State::Missing(k::MissingWorkflow {})
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
            let mutation_operation_id = input
                .mutation_operation_id
                .as_deref()
                .map(id)
                .transpose()?
                .map(nous_core::OperationId);
            self.0
                .store
                .save_model_workflow(
                    &lease,
                    proposal.as_ref(),
                    outcome.as_ref(),
                    execution_telemetry.as_ref(),
                    mutation_operation_id,
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
