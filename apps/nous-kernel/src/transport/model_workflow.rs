// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::model::private_workflow_owner;
use super::workflow_convert as envelope;
use super::*;
use nous_core::Result;
#[tonic::async_trait]
impl k::kernel_model_workflow_service_server::KernelModelWorkflowService for KernelService {
    async fn read_operation(
        &self,
        request: Request<k::OperationIdentity>,
    ) -> std::result::Result<Response<k::OperationRecord>, Status> {
        let result: Result<_> = async {
            let identity = envelope::identity(request.into_inner())?;
            Ok(self
                .0
                .store
                .read_model_operation(
                    identity.subject,
                    &identity.owner,
                    &identity.operation_key,
                    &identity.semantic_digest,
                )
                .await?
                .map(envelope::record)
                .unwrap_or_default())
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn start_operation(
        &self,
        request: Request<k::StartOperationRequest>,
    ) -> std::result::Result<Response<k::OperationRecord>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let identity = envelope::identity(required(input.identity, "operation identity")?)?;
            let subject = identity.subject;
            if identity.owner == "memory" {
                id(&identity.operation_key)?;
                if self.0.memory.is_none() {
                    return Err(Error::Unavailable("Memory owner unavailable".into()));
                }
            }
            private_workflow_owner(&identity.owner)?;
            let mut snapshot = envelope::snapshot(required(input.snapshot, "workflow snapshot")?)?;
            if let Some(claim) = &snapshot.maintenance_trigger {
                if identity.owner != "memory" {
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
                if identity.operation_key != expected.to_string() {
                    return Err(Error::Invalid(
                        "maintenance workflow identity does not match obligation".into(),
                    ));
                }
            }
            if identity.owner == "memory" {
                snapshot.cognitive_formed_at = Some(self.0.cognition.now(subject));
            }
            let record = self
                .0
                .store
                .start_model_operation(
                    subject,
                    &identity.owner,
                    &identity.operation_key,
                    &identity.semantic_digest,
                    &snapshot,
                )
                .await?;
            Ok(envelope::record(record))
        }
        .await;
        result.map(Response::new).map_err(status)
    }
    async fn save_operation(
        &self,
        request: Request<k::SaveOperationRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let identity = envelope::identity(required(input.identity, "operation identity")?)?;
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
                .save_model_operation(
                    &identity,
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
}
