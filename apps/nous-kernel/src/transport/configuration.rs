//! Canonical configuration management, shared by public Core and private Kernel.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_configuration::{ConfigApplyMode, ConfigChangeOutcome, ConfigDescriptor, ConfigExposure};
use nous_core::OperationId;
use p::configuration_service_server::ConfigurationService;

fn ceiling(raw: i32) -> nous_core::Result<u8> {
    match p::ConfigExposure::try_from(raw).map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Configuration,
            nous_core::ErrorCode::InvalidRequest,
            "invalid exposure ceiling",
        )
        .with_source(error)
    })? {
        p::ConfigExposure::Unspecified | p::ConfigExposure::Standard => Ok(1),
        p::ConfigExposure::Advanced => Ok(2),
        p::ConfigExposure::Developer => Ok(3),
    }
}
fn exposure(value: ConfigExposure) -> p::ConfigExposure {
    match value {
        ConfigExposure::Standard => p::ConfigExposure::Standard,
        ConfigExposure::Advanced => p::ConfigExposure::Advanced,
        ConfigExposure::Developer => p::ConfigExposure::Developer,
    }
}
fn visible(d: &ConfigDescriptor, limit: u8, owner: Option<&str>, category: Option<&str>) -> bool {
    exposure(d.exposure) as u8 <= limit
        && owner.is_none_or(|owner| owner == d.owner)
        && category.is_none_or(|category| category == d.category)
}
fn descriptor(d: &ConfigDescriptor) -> p::ConfigDescriptor {
    p::ConfigDescriptor {
        path: d.path.clone(),
        owner: d.owner.clone(),
        title: d.title.clone(),
        description: d.description.clone(),
        category: d.category.clone(),
        json_schema: Some(value(d.json_schema.clone())),
        reference_default: Some(value(d.reference_default.clone())),
        exposure: exposure(d.exposure) as i32,
        scope_policy: enum_name(d.scope_policy),
        storage_policy: enum_name(d.storage_policy),
        apply_mode: enum_name(d.apply_mode),
        semantic_effect: enum_name(d.semantic_effect),
        unit: d.unit.clone(),
        sensitivity: enum_name(d.sensitivity),
        reference_profile: d.reference_profile.clone(),
    }
}
fn change(outcome: ConfigChangeOutcome) -> p::ConfigurationChange {
    let effect = match outcome.apply_mode {
        ConfigApplyMode::Live => "live_applied",
        ConfigApplyMode::RestartProcess => "restart_required",
        ConfigApplyMode::ServingRebuild => "serving_rebuild_required",
        ConfigApplyMode::NewSubjectsOnly => "new_subject_provisioning",
    };
    p::ConfigurationChange {
        revision: outcome.revision,
        apply_mode: enum_name(outcome.apply_mode),
        active_digest: outcome.active_digest,
        desired_digest: outcome.desired_digest,
        effects: vec![effect.into()],
    }
}

fn pending_effect(
    descriptor: &ConfigDescriptor,
    changed: bool,
    projections: Option<&p::ProjectionStatus>,
) -> Option<String> {
    match descriptor.apply_mode {
        ConfigApplyMode::RestartProcess => changed.then(|| "restart_required".into()),
        ConfigApplyMode::ServingRebuild => {
            let Some(projections) = projections else {
                return Some("subject_projection_status_required".into());
            };
            let families = nous_retrieval::affected_families(&[&descriptor.path]);
            if families.is_empty() {
                return Some("projection_status_unavailable".into());
            }
            families
                .iter()
                .any(|family| {
                    let matching = projections
                        .families
                        .iter()
                        .filter(|status| status.name == *family)
                        .collect::<Vec<_>>();
                    matching.is_empty() || matching.iter().any(|status| status.state != "READY")
                })
                .then(|| "serving_rebuild_required".into())
        }
        ConfigApplyMode::Live | ConfigApplyMode::NewSubjectsOnly => None,
    }
}

#[tonic::async_trait]
impl ConfigurationService for KernelService {
    async fn list_config_descriptors(
        &self,
        request: Request<p::ListConfigDescriptorsRequest>,
    ) -> Result<Response<p::ListConfigDescriptorsResponse>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            let limit = ceiling(r.exposure_ceiling)?;
            Ok(p::ListConfigDescriptorsResponse {
                descriptors: self
                    .0
                    .configuration
                    .registry()
                    .descriptors()
                    .filter(|d| visible(d, limit, r.owner.as_deref(), r.category.as_deref()))
                    .map(descriptor)
                    .collect(),
                catalog_digest: self.0.configuration.registry().catalog_digest().into(),
            })
        })
        .await
    }
    async fn get_config_descriptor(
        &self,
        request: Request<p::GetConfigDescriptorRequest>,
    ) -> Result<Response<p::ConfigDescriptor>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            self.0
                .configuration
                .registry()
                .descriptor(&r.path)
                .map(descriptor)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Configuration,
                        nous_core::ErrorCode::NotFound,
                        "configuration descriptor not found",
                    )
                })
        })
        .await
    }
    async fn get_configuration(
        &self,
        request: Request<p::GetConfigurationRequest>,
    ) -> Result<Response<p::ConfigurationSnapshot>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            let limit = ceiling(r.exposure_ceiling)?;
            let subject = r.subject_id.as_deref().map(id).transpose()?.map(SubjectId);
            if let Some(subject) = subject {
                self.0.store.require_subject(subject).await?;
            }
            let active = match subject {
                Some(s) => self.0.configuration.snapshot_for_subject(s)?,
                None => self.0.configuration.active_system_snapshot()?,
            };
            let desired = match subject {
                Some(s) => self.0.configuration.desired_snapshot_for_subject(s)?,
                None => self.0.configuration.desired_system_snapshot()?,
            };
            let snapshot = match p::ConfigurationView::try_from(r.view).map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Configuration,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid configuration view",
                )
                .with_source(error)
            })? {
                p::ConfigurationView::Unspecified | p::ConfigurationView::Active => &active,
                p::ConfigurationView::Desired => &desired,
            };
            for path in &r.paths {
                if self.0.configuration.registry().descriptor(path).is_none() {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Configuration,
                        nous_core::ErrorCode::InvalidRequest,
                        "Configuration path is not registered",
                    )
                    .with_context("path", path));
                }
            }
            let projection_status = match subject {
                Some(subject)
                    if snapshot.descriptors().any(|d| {
                        d.apply_mode == ConfigApplyMode::ServingRebuild
                            && (r.paths.is_empty() || r.paths.contains(&d.path))
                    }) =>
                {
                    Some(
                        self.get_projection_status(p::SubjectRequest {
                            subject_id: subject.0.to_string(),
                        })
                        .await?,
                    )
                }
                _ => None,
            };
            let entries = snapshot
                .descriptors()
                .filter(|d| {
                    (r.paths.is_empty()
                        && visible(d, limit, r.owner.as_deref(), r.category.as_deref()))
                        || r.paths.contains(&d.path)
                })
                .map(|d| p::ConfigurationEntry {
                    path: d.path.clone(),
                    value: snapshot.json(&d.path).cloned().map(value),
                    source: enum_name(snapshot.source(&d.path).expect("resolved source")),
                    owner: d.owner.clone(),
                    apply_mode: enum_name(d.apply_mode),
                    pending_effect: pending_effect(
                        d,
                        active.json(&d.path) != desired.json(&d.path),
                        projection_status.as_ref(),
                    ),
                })
                .collect();
            Ok(p::ConfigurationSnapshot {
                catalog_digest: snapshot.catalog_digest.clone(),
                configuration_revision: snapshot.revision,
                subject_id: r.subject_id,
                entries,
                effective_digest: snapshot.effective_digest.clone(),
            })
        })
        .await
    }
    async fn set_system_override(
        &self,
        request: Request<p::SetSystemOverrideRequest>,
    ) -> Result<Response<p::ConfigurationChange>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            self.0
                .configuration
                .set_system_override(
                    OperationId(id(&r.operation_id)?),
                    &r.path,
                    json(required(r.value, "value")?),
                    Some(required(r.expected_revision, "expected_revision")?),
                )
                .await
                .map(change)
        })
        .await
    }
    async fn clear_system_override(
        &self,
        request: Request<p::ClearSystemOverrideRequest>,
    ) -> Result<Response<p::ConfigurationChange>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            self.0
                .configuration
                .clear_system_override(
                    OperationId(id(&r.operation_id)?),
                    &r.path,
                    Some(required(r.expected_revision, "expected_revision")?),
                )
                .await
                .map(change)
        })
        .await
    }
    async fn set_subject_override(
        &self,
        request: Request<p::SetSubjectOverrideRequest>,
    ) -> Result<Response<p::ConfigurationChange>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            let subject = SubjectId(id(&r.subject_id)?);
            self.0.store.require_subject(subject).await?;
            self.0
                .configuration
                .set_subject_override(
                    OperationId(id(&r.operation_id)?),
                    subject,
                    &r.path,
                    json(required(r.value, "value")?),
                    Some(required(r.expected_revision, "expected_revision")?),
                )
                .await
                .map(change)
        })
        .await
    }
    async fn clear_subject_override(
        &self,
        request: Request<p::ClearSubjectOverrideRequest>,
    ) -> Result<Response<p::ConfigurationChange>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            let subject = SubjectId(id(&r.subject_id)?);
            self.0.store.require_subject(subject).await?;
            self.0
                .configuration
                .clear_subject_override(
                    OperationId(id(&r.operation_id)?),
                    subject,
                    &r.path,
                    Some(required(r.expected_revision, "expected_revision")?),
                )
                .await
                .map(change)
        })
        .await
    }
}

#[tonic::async_trait]
impl k::kernel_configuration_service_server::KernelConfigurationService for KernelService {
    async fn initialize_host_runtime(
        &self,
        request: Request<k::InitializeHostRuntimeRequest>,
    ) -> Result<Response<()>, Status> {
        rpc_reply(async {
            let r = request.into_inner();
            if self
                .0
                .configuration
                .active_system_snapshot()?
                .effective_digest
                != r.configuration_digest
            {
                return Err(nous_core::Error::new(
                    nous_core::ErrorOwner::Configuration,
                    nous_core::ErrorCode::StaleRevision,
                    "startup configuration changed",
                ));
            }
            if let Some(config) = r.resolved_embedding {
                let config: nous_retrieval::StoredEmbeddingConfig =
                    serde_json::from_value(object(Some(config))).map_err(|error| {
                        Error::new(
                            nous_core::ErrorOwner::Configuration,
                            nous_core::ErrorCode::InvalidRequest,
                            "invalid host embedding binding",
                        )
                        .with_source(error)
                    })?;
                self.0.serving.initialize_embedding(config)?;
            }
            Ok(())
        })
        .await
    }
}
