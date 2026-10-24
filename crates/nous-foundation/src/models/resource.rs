use super::{ModelFailure, catalog::*, roles::*};
use crate::configuration::ConfigSnapshot;
use crate::configuration::ConfigSource;
use crate::instance::InstancePaths;
use genai::{
    ModelIden, ServiceTarget,
    adapter::AdapterKind,
    resolver::{AuthData, Endpoint},
};
use nous_domain::{EmbeddingSpace, Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRoute {
    pub id: String,
    pub resource: ResourceMetadata,
    pub parameters: RouteParameters,
    pub input_modalities: Vec<ModelModality>,
    pub capabilities: Vec<ModelCapability>,
    pub embedding: Option<EmbeddingProfile>,
    pub pricing: Option<TokenPricing>,
    provider: Provider,
}
impl ResolvedRoute {
    pub fn space(&self) -> Result<EmbeddingSpace> {
        self.embedding
            .as_ref()
            .map(|profile| embedding_space(&self.resource, profile))
            .ok_or_else(|| Error::invalid("selected model has no embedding space"))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedModel {
    pub scope: nous_domain::ConfigScope,
    pub role: String,
    pub routes: Vec<ResolvedRoute>,
    pub binding: RoleBinding,
    pub config_digest: String,
    pub config_sources: BTreeMap<String, ConfigSource>,
    pub prompt_template: Option<crate::prompts::PromptTemplate>,
}
impl ResolvedModel {
    pub fn primary(&self) -> &ResolvedRoute {
        &self.routes[0]
    }
    pub fn route(&self, attempt: usize) -> &ResolvedRoute {
        &self.routes[attempt.min(self.routes.len() - 1)]
    }
    pub fn space(&self) -> Result<EmbeddingSpace> {
        self.primary().space()
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelReadinessState {
    Ready,
    WaitingConfiguration,
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelReadiness {
    pub role: String,
    pub state: ModelReadinessState,
    pub routes: Vec<String>,
    pub reason: Option<String>,
}
impl ModelReadiness {
    pub fn is_ready(&self) -> bool {
        matches!(self.state, ModelReadinessState::Ready)
    }
}
#[derive(Clone)]
pub struct ModelResources {
    paths: InstancePaths,
    roles: RoleRegistry,
}
impl ModelResources {
    pub fn new(paths: &InstancePaths, roles: RoleRegistry) -> Self {
        Self {
            paths: paths.clone(),
            roles,
        }
    }
    pub fn roles(&self) -> &RoleRegistry {
        &self.roles
    }
    /// Pure resolution: signatures and cache lookup remain available without credentials.
    pub fn resolve(&self, snapshot: &ConfigSnapshot, role: &str) -> Result<ResolvedModel> {
        resolve_registered(snapshot, &self.roles, role)
    }
    pub fn readiness(&self, snapshot: &ConfigSnapshot, role: &str) -> ModelReadiness {
        let resolved = self.resolve(snapshot, role);
        let ready = resolved.as_ref().is_ok_and(|model| {
            model.routes.iter().any(|route| {
                route.provider.enabled && self.paths.read_secret(&route.provider.credential).is_ok()
            })
        });
        ModelReadiness {
            role: role.into(),
            state: if ready {
                ModelReadinessState::Ready
            } else {
                ModelReadinessState::WaitingConfiguration
            },
            routes: resolved
                .as_ref()
                .map(|m| m.routes.iter().map(|r| r.id.clone()).collect())
                .unwrap_or_default(),
            reason: if ready {
                None
            } else {
                Some(resolved.err().map_or_else(
                    || "model credentials are missing, unreadable or provider is disabled".into(),
                    |e| e.message,
                ))
            },
        }
    }
    pub fn ready_resolved(&self, model: &ResolvedModel) -> bool {
        !self.ready_resources(model, 0).is_empty()
    }
    pub(super) fn ready_resources(
        &self,
        model: &ResolvedModel,
        start: usize,
    ) -> Vec<ResourceMetadata> {
        model
            .routes
            .iter()
            .skip(start)
            .filter(|route| {
                route.provider.enabled && self.paths.read_secret(&route.provider.credential).is_ok()
            })
            .map(|route| route.resource.clone())
            .collect()
    }
    pub(super) fn target(
        &self,
        route: &ResolvedRoute,
    ) -> std::result::Result<ServiceTarget, ModelFailure> {
        if !route.provider.enabled {
            return Err(ModelFailure::waiting("provider is disabled"));
        }
        let secret = self
            .paths
            .read_secret(&route.provider.credential)
            .map_err(|error| {
                ModelFailure::waiting("model credential is missing or unreadable")
                    .with_source(error.into_boxed_dyn_error())
            })?;
        Ok(ServiceTarget {
            endpoint: Endpoint::from_owned(route.resource.endpoint.clone()),
            auth: AuthData::from_single(secret),
            model: ModelIden::new(AdapterKind::OpenAI, route.resource.model.clone()),
        })
    }
}

pub fn validate_snapshot(snapshot: &ConfigSnapshot, roles: &RoleRegistry) -> Result<()> {
    for role in roles.definitions() {
        if snapshot.get(role.binding_key)?.is_some() {
            resolve_registered(snapshot, roles, role.id)?;
        }
    }
    Ok(())
}
fn resolve_registered(
    snapshot: &ConfigSnapshot,
    roles: &RoleRegistry,
    role: &str,
) -> Result<ResolvedModel> {
    let definition = roles.get(role)?;
    let catalog = snapshot.get(CATALOG)?;
    catalog.validate()?;
    let binding = snapshot.get(definition.binding_key)?.ok_or_else(|| {
        Error::new(
            nous_domain::ErrorCode::CapabilityUnavailable,
            "model role is not configured",
        )
    })?;
    binding.validate()?;
    binding.prompt.validate_for_scope(&snapshot.scope)?;
    let mut routes = Vec::new();
    for id in &binding.routes {
        let route = catalog
            .routes
            .get(id)
            .ok_or_else(|| Error::invalid("role references a missing route"))?;
        let model = &catalog.models[&route.model];
        let provider = &catalog.providers[&model.provider];
        if !definition
            .requirements
            .input_modalities
            .iter()
            .all(|m| model.input_modalities.contains(m))
            || !definition
                .requirements
                .capabilities
                .iter()
                .all(|c| model.capabilities.contains(c))
        {
            return Err(Error::invalid(
                "route model does not meet the registered role requirements",
            ));
        }
        routes.push(ResolvedRoute {
            id: id.clone(),
            resource: ResourceMetadata {
                id: route.model.clone(),
                provider: model.provider.clone(),
                endpoint: provider.endpoint.clone(),
                protocol: model.protocol,
                model: model.request_model.clone(),
            },
            parameters: route.parameters.clone(),
            input_modalities: model.input_modalities.clone(),
            capabilities: model.capabilities.clone(),
            embedding: model.embedding.clone(),
            pricing: model.pricing.clone(),
            provider: provider.clone(),
        });
    }
    // Fallback preserves the complete immutable vector space.
    if routes[0].embedding.is_some() {
        let expected = routes[0].space()?;
        if routes
            .iter()
            .skip(1)
            .any(|route| !route.space().is_ok_and(|space| space == expected))
        {
            return Err(Error::invalid(
                "embedding routes require identical complete space signatures",
            ));
        }
    }
    let keys = [CATALOG.path(), definition.binding_key.path()];
    Ok(ResolvedModel {
        scope: snapshot.scope.clone(),
        role: role.into(),
        routes,
        binding,
        prompt_template: None,
        config_digest: snapshot.digest_for(&keys)?,
        config_sources: keys
            .iter()
            .filter_map(|key| {
                snapshot
                    .source(key)
                    .cloned()
                    .map(|source| ((*key).into(), source))
            })
            .collect(),
    })
}
