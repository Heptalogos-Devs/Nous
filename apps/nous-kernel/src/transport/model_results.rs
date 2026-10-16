// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::model_result_codec as envelope;
use super::*;
use nous_core::Result;
#[tonic::async_trait]
impl k::kernel_model_results_service_server::KernelModelResultsService for KernelService {
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
    async fn save_operation(
        &self,
        request: Request<k::SaveOperationRequest>,
    ) -> std::result::Result<Response<()>, Status> {
        let input = request.into_inner();
        let result: Result<_> = async {
            let identity = envelope::identity(required(input.identity, "operation identity")?)?;
            let mut context = input.context.map(envelope::context).transpose()?;
            if identity.owner == "memory" && input.proposal.is_some() {
                context.get_or_insert_with(Default::default);
            }
            if let Some(context) = &mut context {
                if let Some(claim) = &context.maintenance_trigger {
                    let expected = uuid::Uuid::new_v5(
                        &uuid::Uuid::NAMESPACE_OID,
                        format!(
                            "{}:{}:{}",
                            claim.need_id, claim.trigger_authority_seq, claim.trigger_revision
                        )
                        .as_bytes(),
                    );
                    if !["memory", "subject_reflection"].contains(&identity.owner.as_str())
                        || identity.operation_key != expected.to_string()
                    {
                        return Err(Error::Invalid(
                            "maintenance result identity does not match obligation".into(),
                        ));
                    }
                }
                if identity.owner == "memory" && input.proposal.is_some() {
                    id(&identity.operation_key)?;
                    context.cognitive_formed_at =
                        Some(self.0.cognition.read_clock(identity.subject).await?);
                }
            }
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
                    context.as_ref(),
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
