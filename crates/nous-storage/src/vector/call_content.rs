//! Read the actual call's immutable content ordering, including explicit erased entries.
use crate::{WriteTransaction, storage_error};
use nous_domain::*;
use serde_json::Value;
use std::collections::HashSet;

impl WriteTransaction {
    pub(super) async fn embedding_calls_for_content(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space: &str,
        hash: &str,
    ) -> Result<Vec<ModelCall>> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM model_calls WHERE namespace_id=? AND subject_id=? AND json_extract(request,'$.embedding_space.id')=? AND json_extract(request,'$.preview.input.kind')='embedding'")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut calls = Vec::new();
        for id in ids {
            let call = self.model_call(&ModelCallId(id)).await?;
            let Some(inputs) = call
                .request
                .pointer("/input/inputs")
                .and_then(Value::as_array)
            else {
                continue;
            };
            if inputs
                .iter()
                .map(input_hash)
                .collect::<Result<Vec<_>>>()?
                .iter()
                .any(|value| value == hash)
            {
                calls.push(call);
            }
        }
        Ok(calls)
    }
}
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
    if input.get("redacted") == Some(&Value::Bool(true))
        || input.get("source_redacted") == Some(&Value::Bool(true))
    {
        input
            .get("source_hash")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| Error::internal("redacted embedding input has no retained hash"))
    } else {
        Ok(serde_json::from_value::<EmbeddingInputItem>(input.clone())?.source_hash())
    }
}
