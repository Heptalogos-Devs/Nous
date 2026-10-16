// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_protocol::kernel::kernel_configuration_service_server::KernelConfigurationServiceServer;
use nous_protocol::kernel::{
    artifact_stream_service_server::ArtifactStreamServiceServer,
    kernel_material_workflow_service_server::KernelMaterialWorkflowServiceServer,
    kernel_model_results_service_server::KernelModelResultsServiceServer,
};
use nous_protocol::public::configuration_service_server::ConfigurationServiceServer;

const WORKFLOW_PROTOCOL_ENVELOPE_BYTES: usize = 65536;

pub async fn router(runtime: NousRuntime, token: String) -> tonic::transport::server::Router {
    let auth = move |request: Request<()>| -> std::result::Result<Request<()>, Status> {
        let value = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        let expected = format!("Bearer {token}");
        if value.len() != expected.len()
            || value
                .bytes()
                .zip(expected.bytes())
                .fold(0u8, |a, (x, y)| a | (x ^ y))
                != 0
        {
            return Err(Status::unauthenticated("invalid Kernel credential"));
        }
        Ok(request)
    };
    let service = KernelService(runtime);
    let (reporter, health) = tonic_health::server::health_reporter();
    reporter
        .set_serving::<nous_protocol::kernel::kernel_query_service_server::KernelQueryServiceServer<KernelService>>()
        .await;
    tonic::transport::Server::builder()
        .add_service(nous_protocol::public::cognitive_clock_service_server::CognitiveClockServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::public::self_service_server::SelfServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(KernelConfigurationServiceServer::with_interceptor(
            service.clone(),
            auth.clone(),
        ))
        .add_service(
            nous_protocol::public::material_service_server::MaterialServiceServer::with_interceptor(
                service.clone(),
                auth.clone(),
            ),
        )
        .add_service(
            nous_protocol::public::subject_service_server::SubjectServiceServer::with_interceptor(
                service.clone(),
                auth.clone(),
            ),
        )
        .add_service(
            nous_protocol::public::memory_service_server::MemoryServiceServer::with_interceptor(
                service.clone(),
                auth.clone(),
            ),
        )
        .add_service(
            nous_protocol::public::concept_service_server::ConceptServiceServer::with_interceptor(
                service.clone(),
                auth.clone(),
            ),
        )
        .add_service(
            nous_protocol::public::identity_service_server::IdentityServiceServer::with_interceptor(
                service.clone(),
                auth.clone(),
            ),
        )
        .add_service(nous_protocol::public::social_service_server::SocialServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::public::runtime_service_server::RuntimeServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::public::resource_registry_service_server::ResourceRegistryServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::public::system_service_server::SystemServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::kernel::kernel_query_service_server::KernelQueryServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::kernel::kernel_maintenance_service_server::KernelMaintenanceServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(nous_protocol::kernel::kernel_projection_service_server::KernelProjectionServiceServer::with_interceptor(service.clone(),auth.clone()))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            KernelMaterialWorkflowServiceServer::new(service.clone())
                .max_decoding_message_size(nous_persistence::MODEL_RESULT_MAX_BYTES + WORKFLOW_PROTOCOL_ENVELOPE_BYTES)
                .max_encoding_message_size(nous_persistence::MODEL_RESULT_MAX_BYTES + WORKFLOW_PROTOCOL_ENVELOPE_BYTES),
            auth.clone(),
        ))
        .add_service(ConfigurationServiceServer::with_interceptor(
            service.clone(),
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            KernelModelResultsServiceServer::new(service.clone())
                .max_decoding_message_size(
                    nous_persistence::MODEL_RESULT_MAX_BYTES + WORKFLOW_PROTOCOL_ENVELOPE_BYTES,
                )
                .max_encoding_message_size(
                    nous_persistence::MODEL_RESULT_MAX_BYTES + WORKFLOW_PROTOCOL_ENVELOPE_BYTES,
                ),
            auth.clone(),
        ))
        .add_service(ArtifactStreamServiceServer::with_interceptor(
            service,
            auth.clone(),
        ))
        .add_service(tonic::service::interceptor::InterceptedService::new(
            health, auth,
        ))
}
