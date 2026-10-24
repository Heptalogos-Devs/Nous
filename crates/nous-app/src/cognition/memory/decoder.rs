use nous_domain::{ErrorCode, MemoryDraft};
use nous_foundation::models::ModelFailure;
use nous_foundation::models::RetryClass;
use serde::Deserialize;
use serde_json::{Value, json};
pub(super) fn output_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["memories"],"properties":{
        "memories":{"type":"array","minItems":1,"maxItems":3,"items":{"type":"object","additionalProperties":false,
            "required":["title","content","epistemic_status"],"properties":{
                "title":{"type":"string","minLength":1,"maxLength":256},
                "content":{"type":"string","minLength":1,"maxLength":16384},
                "epistemic_status":{"type":"string","enum":["reported","inferred","uncertain"]}}}}}})
}

pub(super) fn parse_drafts(text: &str) -> Result<Vec<MemoryDraft>, ModelFailure> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Output {
        memories: Vec<Draft>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Draft {
        title: String,
        content: String,
        epistemic_status: String,
    }
    let output: Output = serde_json::from_str(text).map_err(|error| {
        let failure = ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output does not match the formation JSON contract",
        );
        failure.with_source(Box::new(error))
    })?;
    if !(1..=3).contains(&output.memories.len())
        || output.memories.iter().any(|draft| {
            draft.title.trim().is_empty()
                || draft.title.chars().count() > 256
                || draft.content.trim().is_empty()
                || draft.content.chars().count() > 16384
                || !matches!(
                    draft.epistemic_status.as_str(),
                    "reported" | "inferred" | "uncertain"
                )
        })
    {
        return Err(ModelFailure::new(
            RetryClass::Permanent,
            ErrorCode::InvalidArgument,
            "model output must contain 1 to 3 bounded memories with valid epistemic status",
        ));
    }
    Ok(output
        .memories
        .into_iter()
        .map(|draft| MemoryDraft {
            title: draft.title,
            content: draft.content,
            epistemic_status: draft.epistemic_status,
        })
        .collect())
}
