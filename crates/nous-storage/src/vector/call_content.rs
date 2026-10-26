//! Read the actual call's immutable content ordering, including explicit erased entries.
use nous_domain::*;
use serde_json::Value;
use std::collections::HashSet;
#[derive(Debug, Clone)]
pub struct EmbeddingCallContent {
    pub source_hash: String,
    pub vector: Option<Vec<f32>>,
}
pub fn embedding_call_contents(call: &ModelCall) -> Result<Vec<EmbeddingCallContent>> {
    let inputs = call
        .request
        .pointer("/input/inputs")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("embedding call has no actual input directory"))?;
    let results = call
        .response
        .as_ref()
        .and_then(|value| value.get("result"))
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("embedding call has no canonical vector result"))?;
    if inputs.len() != results.len() {
        return Err(Error::internal(
            "embedding actual input and vector counts disagree",
        ));
    }
    let mut hashes = HashSet::new();
    inputs
        .iter()
        .zip(results)
        .map(|(input, result)| {
            let redacted = input.get("redacted") == Some(&Value::Bool(true));
            let hash = input_hash(input)?;
            validate_embedding_hash(&hash)?;
            if !hashes.insert(hash.clone()) {
                return Err(Error::invalid(
                    "actual embedding call repeats semantic content",
                ));
            }
            if redacted && !result.is_null() {
                return Err(Error::internal(
                    "redacted embedding content regained a vector payload",
                ));
            }
            let vector = if result.is_null() {
                None
            } else {
                Some(serde_json::from_value(result.clone())?)
            };
            Ok(EmbeddingCallContent {
                source_hash: hash,
                vector,
            })
        })
        .collect()
}
pub(super) fn input_hash(input: &Value) -> Result<String> {
    if input.get("redacted") == Some(&Value::Bool(true)) {
        input
            .get("source_hash")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::internal("redacted embedding input has no retained hash"))
    } else {
        Ok(serde_json::from_value::<EmbeddingInputItem>(input.clone())?.source_hash())
    }
}
