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
            let Some(row) = row else {
                return Ok(k::FoundWorkflow::default());
            };
            Ok(k::FoundWorkflow {
                found: true,
                snapshot: Some(envelope::snapshot_proto(row.snapshot)),
                proposal: row.proposal.map(envelope::payload_proto),
                outcome: row.outcome.map(envelope::payload_proto),
                execution_telemetry: row.execution_telemetry.map(envelope::telemetry_proto),
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
                    return Err(Error::Unavailable("Memory owner unavailable".into()));
                }
            }
            private_workflow_owner(&input.owner)?;
            let mut snapshot = envelope::snapshot(required(input.snapshot, "workflow snapshot")?)?;
            if let Some(claim) = &snapshot.maintenance_claim {
                if input.owner != "memory" {
                    return Err(Error::Invalid(
                        "maintenance workflow route requires Memory".into(),
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
                    return Err(Error::Invalid(
                        "maintenance workflow identity does not match obligation".into(),
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
            Ok(k::WorkflowReservation {
                snapshot: Some(envelope::snapshot_proto(reserved.snapshot)),
                proposal: reserved.proposal.map(envelope::payload_proto),
                outcome: reserved.outcome.map(envelope::payload_proto),
                lease: reserved.lease.map(envelope::lease_proto),
                busy: reserved.busy,
                execution_telemetry: reserved.execution_telemetry.map(envelope::telemetry_proto),
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
