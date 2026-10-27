use crate::ReadView;
use nous_domain::*;
use serde_json::Value;
use std::collections::BTreeSet;

impl ReadView {
    pub async fn readable_model_call(
        &mut self,
        ctx: &CallContext,
        id: &ModelCallId,
    ) -> Result<ModelCall> {
        let call = self.model_call(id).await?;
        self.require_model_call_sources(ctx, &call).await?;
        Ok(call)
    }
    pub async fn require_model_call_sources(
        &mut self,
        ctx: &CallContext,
        call: &ModelCall,
    ) -> Result<()> {
        ctx.require_subject(&call.namespace_id, &call.subject_id)?;
        let erased = flag(&call.request, "redacted");
        let Some(sources) = call
            .request
            .pointer("/preview/sources")
            .filter(|value| !value.is_null())
        else {
            if ctx.source_read.is_empty() && (erased || call.origin == ModelCallOrigin::Local) {
                return Ok(());
            }
            return Err(Error::new(
                ErrorCode::Forbidden,
                "model history has no attributable source catalog",
            ));
        };
        let sources: Vec<SourceReadRef> = serde_json::from_value(sources.clone())?;
        let (bodies, facts) = source_access(call, &sources)?;
        let bodies = self.imported_model_sources(call, &bodies).await?;
        let facts = self.imported_model_sources(call, &facts).await?;
        if !bodies.is_empty() {
            self.require_source_requirements(ctx, &call.namespace_id, &call.subject_id, &bodies)
                .await?;
        }
        if !facts.is_empty() {
            self.require_source_provenance(ctx, &call.namespace_id, &call.subject_id, &facts)
                .await?;
        }
        Ok(())
    }
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key) == Some(&Value::Bool(true))
}
fn source_access(
    call: &ModelCall,
    sources: &[SourceReadRef],
) -> Result<(Vec<SourceReadRef>, Vec<SourceReadRef>)> {
    if flag(&call.request, "redacted") {
        return Ok((vec![], sources.to_vec()));
    }
    let mut facts = BTreeSet::new();
    let mut live = BTreeSet::new();
    if call
        .request
        .pointer("/preview/input/kind")
        .and_then(Value::as_str)
        == Some("embedding")
    {
        let inputs = call
            .request
            .pointer("/input/inputs")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::internal("embedding history has no actual input catalog"))?;
        for input in inputs {
            if flag(input, "source_redacted") || flag(input, "redacted") {
                let requirements: Vec<SourceReadRef> =
                    serde_json::from_value(input.get("source_requirements").cloned().ok_or_else(
                        || Error::internal("erased embedding input has no source facts"),
                    )?)?;
                facts.extend(requirements);
            } else {
                let actual: EmbeddingInputItem = serde_json::from_value(input.clone())?;
                live.extend(actual.source_requirements());
            }
        }
    }
    // A source used by any still-present item remains a body dependency.
    facts.retain(|source| !live.contains(source));
    let bodies = sources
        .iter()
        .filter(|source| !facts.contains(*source))
        .cloned()
        .collect();
    let facts = sources
        .iter()
        .filter(|source| facts.contains(*source))
        .cloned()
        .collect();
    Ok((bodies, facts))
}
