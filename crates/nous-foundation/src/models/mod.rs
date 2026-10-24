//! Shared model resources and SDK-independent invocation preparation/execution.
//! Domain consumers own their prompt arguments, input/output DTOs and adoption.
mod catalog;
mod contract;
mod error;
mod execution;
mod input;
mod preview;
mod request;
mod resource;
mod roles;
pub use catalog::*;
pub use contract::{InvocationDraft, RoleContract, RoleContractView};
pub use error::{ModelFailure, ModelFailureInfo, RetryClass};
pub use execution::{
    AttemptContext, AttemptPolicy, CallMetadata, CallReservation, ModelCallRecord, ModelExecution,
    PreparedCall,
};
pub use input::{
    InvocationEdits, InvocationMetadata, InvocationPreview, MediaData, MessagePart, MessageRole,
    ModelInput, ModelMessage, ModelOutput, OutputContract, RoutePreview, SourceAccess,
};
pub use resource::{
    ModelReadiness, ModelReadinessState, ModelResources, ResolvedModel, ResolvedRoute,
    validate_snapshot,
};
pub use roles::{
    CATALOG, ModelRequirements, ModelsDeployment, RetryPolicy, RoleBinding, RoleDefinition,
    RoleRegistry, RoleRegistryBuilder, register_configuration,
};

use crate::configuration::Configuration;
use crate::instance::InstancePaths;
use crate::prompts::Prompts;
use execution::{CallExecutor, ProviderOutput};
use nous_domain::ConfigScope;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct ModelRuntime {
    configuration: Configuration,
    resources: ModelResources,
    prompts: Prompts,
    executor: CallExecutor,
}
impl ModelRuntime {
    pub fn new(
        paths: &InstancePaths,
        configuration: Configuration,
        roles: RoleRegistry,
        http: reqwest::Client,
    ) -> anyhow::Result<Self> {
        let prompts = Prompts::with_definitions(
            paths.clone(),
            roles.definitions().filter_map(|r| r.prompt.clone()),
        );
        Ok(Self {
            configuration,
            resources: ModelResources::new(paths, roles),
            prompts,
            executor: CallExecutor::new(http)?,
        })
    }
    pub fn resources(&self) -> &ModelResources {
        &self.resources
    }
    pub fn prompts(&self) -> &Prompts {
        &self.prompts
    }
    pub async fn resolve(
        &self,
        scope: &ConfigScope,
        role: &str,
    ) -> Result<ResolvedModel, ModelFailure> {
        let snapshot = self
            .configuration
            .snapshot(scope, false)
            .await
            .map_err(ModelFailure::domain)?;
        let mut resolved = self
            .resources
            .resolve(&snapshot, role)
            .map_err(|error| ModelFailure::waiting(error.message))?;
        if let Some(prompt) = &self
            .resources
            .roles()
            .get(role)
            .map_err(ModelFailure::domain)?
            .prompt
        {
            resolved.prompt_template = Some(
                self.prompts
                    .read(prompt.id, &resolved.binding.prompt)
                    .map_err(ModelFailure::prompt)?,
            );
        }
        Ok(resolved)
    }
    pub async fn ready(&self, scope: &ConfigScope, role: &str) -> bool {
        let Ok(snapshot) = self.configuration.snapshot(scope, false).await else {
            return false;
        };
        if !self.resources.readiness(&snapshot, role).is_ready() {
            return false;
        }
        let Ok(resolved) = self.resources.resolve(&snapshot, role) else {
            return false;
        };
        match self
            .resources
            .roles()
            .get(role)
            .ok()
            .and_then(|r| r.prompt.as_ref())
        {
            Some(definition) => self
                .prompts
                .read(definition.id, &resolved.binding.prompt)
                .is_ok(),
            None => true,
        }
    }
    pub async fn inspect(&self, scope: &ConfigScope) -> nous_domain::Result<Value> {
        let snapshot = self.configuration.snapshot(scope, false).await?;
        let mut statuses = serde_json::Map::new();
        for definition in self.resources.roles().definitions() {
            let ready = self.resources.readiness(&snapshot, definition.id);
            let mut value = json!(ready);
            if let Ok(resolved) = self.resources.resolve(&snapshot, definition.id) {
                value["resolved"] = json!(resolved);
                if let Some(prompt) = &definition.prompt {
                    match self.prompts.read(prompt.id, &resolved.binding.prompt) {
                        Ok(asset) => value["prompt"] = json!(asset),
                        Err(error) => {
                            value["state"] = json!("waiting_configuration");
                            value["reason"] = json!(error.message);
                        }
                    }
                }
            }
            statuses.insert(definition.id.into(), value);
        }
        Ok(Value::Object(statuses))
    }
    pub async fn contract(
        &self,
        scope: &ConfigScope,
        role: &str,
        input: Option<Value>,
    ) -> nous_domain::Result<RoleContractView> {
        let definition = self.resources.roles().get(role)?;
        let draft = input
            .as_ref()
            .map(|input| definition.contract.build(input))
            .transpose()?;
        let snapshot = self.configuration.snapshot(scope, false).await?;
        let binding = snapshot.get(definition.binding_key)?;
        let prompt = definition.prompt.as_ref().map(|definition| {
            let prompt_binding = binding
                .as_ref()
                .map(|binding| binding.prompt.clone())
                .unwrap_or_default();
            self.prompts.read(definition.id, &prompt_binding)
        });
        let (prompt, prompt_error) = match prompt {
            Some(Ok(prompt)) => (Some(prompt), None),
            Some(Err(error)) => (None, Some(error)),
            None => (None, None),
        };
        Ok(RoleContractView {
            role: role.into(),
            input_schema: definition.contract.input_schema.clone(),
            requirements: definition.requirements.clone(),
            prompt_id: definition.prompt.as_ref().map(|prompt| prompt.id.into()),
            binding,
            binding_source: snapshot.source(definition.binding_key.path()).cloned(),
            prompt,
            prompt_error,
            draft,
        })
    }
    pub async fn preview(
        &self,
        scope: &ConfigScope,
        role: &str,
        input: Value,
        edits: InvocationEdits,
    ) -> Result<InvocationPreview, ModelFailure> {
        let model = self.resolve(scope, role).await?;
        Ok(preview::build(self, &model, &input, edits)?.preview)
    }
    pub fn plan(&self, scope: ConfigScope, role: &str) -> InvocationPlan {
        InvocationPlan {
            runtime: self.clone(),
            scope,
            role: role.into(),
            resolved: tokio::sync::OnceCell::new(),
        }
    }
    pub async fn ready_plan(
        &self,
        scope: &ConfigScope,
        role: &str,
        attempt: AttemptContext,
    ) -> bool {
        let plan = self.plan(scope.clone(), role);
        let Ok(resolved) = plan.resolve(&attempt).await else {
            return false;
        };
        self.resources.ready_resolved(resolved)
    }
    /// A single attempt freezes metadata and resolves its credential only after cache reuse.
    fn prepare<T: Serialize>(
        &self,
        model: &ResolvedModel,
        attempt: &AttemptContext,
        input: T,
        edits: InvocationEdits,
    ) -> Result<PreparedCall<InvocationMetadata<T>>, ModelFailure> {
        let source =
            serde_json::to_value(&input).map_err(|error| ModelFailure::domain(error.into()))?;
        let built = preview::build(self, model, &source, edits)?;
        let start = if let Some(previous) = &attempt.previous_request {
            let previous: InvocationMetadata<Value> = serde_json::from_value(previous.clone())
                .map_err(|error| {
                    ModelFailure::domain(nous_domain::Error::invalid(
                        "saved invocation metadata is invalid",
                    ))
                    .with_source(Box::new(error))
                })?;
            let position = model
                .routes
                .iter()
                .position(|route| route.id == previous.route)
                .ok_or_else(|| {
                    ModelFailure::domain(nous_domain::Error::invalid(
                        "saved attempt route is absent from its plan",
                    ))
                })?;
            (position + 1).min(model.routes.len() - 1)
        } else {
            0
        };
        let mut selected = None;
        let mut unavailable = ModelFailure::waiting("no executable model route is available");
        for (index, request) in built.requests.into_iter().enumerate().skip(start) {
            let Some(request) = request else {
                if let Some(error) = &built.preview.routes[index].rejection {
                    unavailable = ModelFailure::domain(error.clone());
                }
                continue;
            };
            match self.resources.target(&model.routes[index]) {
                Ok(target) => {
                    selected = Some((index, request, target));
                    break;
                }
                Err(error) => unavailable = error,
            }
        }
        let (index, request, target) = selected.ok_or(unavailable)?;
        let route = &model.routes[index];
        let access = built.preview.routes[index].actual_access;
        let metadata = InvocationMetadata {
            call_id: uuid::Uuid::new_v4().to_string(),
            role: model.role.clone(),
            resource: route.resource.clone(),
            route: route.id.clone(),
            config_digest: model.config_digest.clone(),
            config_sources: model.config_sources.clone(),
            input,
            preview: built.preview,
            actual_access: access,
            max_calls_per_day: model.binding.max_calls_per_day,
            plan: model.clone(),
        };
        Ok(PreparedCall {
            metadata,
            target,
            request,
            attempt_policy: AttemptPolicy {
                max_attempts: model.binding.retry.max_attempts,
                has_fallback: index + 1 < model.routes.len(),
                retry_uncertain: model.binding.retry.retry_uncertain,
            },
        })
    }
    pub async fn execute<M>(&self, prepared: PreparedCall<M>) -> ModelExecution<M, ModelOutput> {
        self.executor.invoke(prepared).await.map(|output, _| {
            Ok(match output {
                ProviderOutput::Text(text) => ModelOutput::Text(text),
                ProviderOutput::Vectors(vectors) => ModelOutput::Vectors(vectors),
            })
        })
    }
}
/// One invocation pins its first successfully resolved resource/Prompt snapshot across attempts.
pub struct InvocationPlan {
    runtime: ModelRuntime,
    scope: ConfigScope,
    role: String,
    resolved: tokio::sync::OnceCell<ResolvedModel>,
}
impl InvocationPlan {
    pub async fn resolve(&self, attempt: &AttemptContext) -> Result<&ResolvedModel, ModelFailure> {
        self.resolved
            .get_or_try_init(|| async {
                if let Some(previous) = &attempt.previous_request {
                    let metadata: InvocationMetadata<Value> =
                        serde_json::from_value(previous.clone()).map_err(|e| {
                            ModelFailure::domain(nous_domain::Error::invalid(
                                "saved invocation plan is invalid",
                            ))
                            .with_source(Box::new(e))
                        })?;
                    if metadata.plan.role != self.role
                        || metadata.plan.scope != self.scope
                        || metadata.plan.routes.is_empty()
                    {
                        return Err(ModelFailure::domain(nous_domain::Error::invalid(
                            "saved invocation plan differs from task scope or role",
                        )));
                    }
                    return Ok(metadata.plan);
                }
                self.runtime.resolve(&self.scope, &self.role).await
            })
            .await
    }
    pub async fn preview<T: Serialize>(
        &self,
        attempt: &AttemptContext,
        input: &T,
        edits: InvocationEdits,
    ) -> Result<InvocationPreview, ModelFailure> {
        let source =
            serde_json::to_value(input).map_err(|error| ModelFailure::domain(error.into()))?;
        Ok(preview::build(&self.runtime, self.resolve(attempt).await?, &source, edits)?.preview)
    }
    pub async fn prepare<T: Serialize>(
        &self,
        attempt: AttemptContext,
        input: T,
        edits: InvocationEdits,
    ) -> Result<PreparedCall<InvocationMetadata<T>>, ModelFailure> {
        self.runtime
            .prepare(self.resolve(&attempt).await?, &attempt, input, edits)
    }
}
