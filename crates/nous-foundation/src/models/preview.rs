//! One provider-free builder feeds both review and actual invocation admission.
use super::{
    InvocationEdits, InvocationPreview, ModelFailure, ModelInput, ModelMessage, ModelRuntime,
    OutputContract, ResolvedModel, RoutePreview, request,
};
use nous_domain::Error;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) struct BuiltInvocation {
    pub preview: InvocationPreview,
    pub requests: Vec<Option<request::ValidatedRequest>>,
}
pub(super) fn build(
    runtime: &ModelRuntime,
    model: &ResolvedModel,
    input: &Value,
    edits: InvocationEdits,
) -> Result<BuiltInvocation, ModelFailure> {
    let role = runtime
        .resources()
        .roles()
        .get(&model.role)
        .map_err(ModelFailure::domain)?;
    let draft = role.contract.build(input).map_err(ModelFailure::domain)?;
    let (arguments, mut invocation) = draft.input();
    let prompt = if let Some(template) = &model.prompt_template {
        Some(
            runtime
                .prompts()
                .preview_template(template, &arguments, edits.prompt.as_deref())
                .map_err(ModelFailure::prompt)?,
        )
    } else {
        if edits.prompt.is_some() || !arguments.is_empty() {
            return Err(ModelFailure::domain(Error::invalid(
                "role has no instruction template for supplied prompt edits or variables",
            )));
        }
        None
    };
    if let Some(prompt) = &prompt {
        let ModelInput::Chat { messages, .. } = &mut invocation else {
            return Err(ModelFailure::domain(Error::invalid(
                "embedding role cannot bind a chat instruction template",
            )));
        };
        messages.insert(0, ModelMessage::system(prompt.text.clone()));
    }
    request::normalize(&mut invocation);
    let schema_hash = match &invocation {
        ModelInput::Chat {
            output: OutputContract::JsonSchema { schema, .. },
            ..
        } => Some(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(schema).map_err(|e| ModelFailure::domain(e.into()))?)
        )),
        _ => None,
    };
    let mut routes = Vec::new();
    let mut requests = Vec::new();
    for route in &model.routes {
        let prepared = request::prepare(route, &invocation);
        let (request, access, rejection) = match prepared {
            Ok((request, access)) => (Some(request), access, None),
            Err(error) => (None, request::no_access(), Some(error)),
        };
        routes.push(RoutePreview {
            route: route.id.clone(),
            resource: route.resource.clone(),
            parameters: route.parameters.clone(),
            actual_access: access,
            rejection,
        });
        requests.push(request);
    }
    Ok(BuiltInvocation {
        preview: InvocationPreview {
            role: model.role.clone(),
            config_digest: model.config_digest.clone(),
            prompt,
            input: invocation,
            output_schema_sha256: schema_hash,
            edits,
            routes,
        },
        requests,
    })
}
