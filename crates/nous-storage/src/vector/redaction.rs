//! Source bodies and independently owned mathematical results have distinct lifetimes.
use crate::{CallDocument, CallRedaction, WriteTransaction};
use nous_domain::*;
use serde_json::{Value, json};
use std::collections::BTreeSet;

impl WriteTransaction {
    pub(crate) async fn redact_embedding_sources(
        &mut self,
        call: &ModelCall,
        targets: &BTreeSet<RetrievalTarget>,
    ) -> Result<()> {
        let Some(inputs) = inputs(call) else {
            return Ok(());
        };
        let mut masks = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            if is_erased(input) || source_erased(input) {
                continue;
            }
            let actual: EmbeddingInputItem = serde_json::from_value(input.clone())?;
            let mapped = self
                .imported_model_sources(call, &actual.source_requirements())
                .await?;
            if mapped
                .iter()
                .filter_map(SourceReadRef::retrieval_target)
                .any(|target| targets.contains(&target))
            {
                masks.extend(item_masks(index, tombstone(input, false)?, false));
            }
        }
        self.apply_embedding_masks(call, masks, "embedding_source_purged")
            .await
    }
    pub(super) async fn redact_embedding_hash(
        &mut self,
        call: &ModelCall,
        hash: &str,
    ) -> Result<()> {
        let Some(inputs) = inputs(call) else {
            return Ok(());
        };
        let mut masks = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            if super::call_content::input_hash(input)? == hash {
                masks.extend(item_masks(index, tombstone(input, true)?, true));
            }
        }
        self.apply_embedding_masks(call, masks, "embedding_content_purged")
            .await
    }
    async fn apply_embedding_masks(
        &mut self,
        call: &ModelCall,
        mut masks: Vec<CallRedaction>,
        reason: &str,
    ) -> Result<()> {
        if masks.is_empty() {
            return Ok(());
        }
        for pointer in [
            "/record/raw_output",
            "/record/provider_response",
            "/record/failure",
        ] {
            masks.push(CallRedaction {
                document: CallDocument::Response,
                pointer: pointer.into(),
                replacement: Value::Null,
            });
        }
        masks.push(CallRedaction {
            document: CallDocument::Error,
            pointer: String::new(),
            replacement: Value::String("model error detail was permanently redacted".into()),
        });
        self.redact_model_call(&call.id, &masks, reason).await
    }
}
fn inputs(call: &ModelCall) -> Option<&[Value]> {
    call.request
        .pointer("/input/inputs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
}
fn is_erased(input: &Value) -> bool {
    input.get("redacted") == Some(&Value::Bool(true))
}
fn source_erased(input: &Value) -> bool {
    input.get("source_redacted") == Some(&Value::Bool(true))
}
fn tombstone(input: &Value, erase_content: bool) -> Result<Value> {
    let mut value = if is_erased(input) || source_erased(input) {
        input.clone()
    } else {
        let actual: EmbeddingInputItem = serde_json::from_value(input.clone())?;
        let blobs = actual
            .parts
            .iter()
            .filter_map(|part| match part {
                EmbeddingPart::Media { blob, .. } => Some(blob),
                _ => None,
            })
            .collect::<Vec<_>>();
        json!({"target":actual.target,"source_hash":actual.source_hash(),"source_requirements":actual.source_requirements(),"blob_refs":blobs,"source_redacted":true})
    };
    value["reason"] = json!(if erase_content {
        "no_remaining_content_consumers"
    } else {
        "source_permanently_purged"
    });
    if erase_content {
        value["redacted"] = json!(true);
    }
    Ok(value)
}
fn item_masks(index: usize, tombstone: Value, erase_content: bool) -> Vec<CallRedaction> {
    let mut masks = vec![
        CallRedaction {
            document: CallDocument::Request,
            pointer: format!("/input/inputs/{index}"),
            replacement: tombstone.clone(),
        },
        CallRedaction {
            document: CallDocument::Request,
            pointer: format!("/preview/input/inputs/{index}"),
            replacement: json!([]),
        },
        CallRedaction {
            document: CallDocument::Response,
            pointer: format!("/record/metadata/input/inputs/{index}"),
            replacement: tombstone,
        },
        CallRedaction {
            document: CallDocument::Response,
            pointer: format!("/record/metadata/preview/input/inputs/{index}"),
            replacement: json!([]),
        },
    ];
    if erase_content {
        masks.push(CallRedaction {
            document: CallDocument::Response,
            pointer: format!("/result/{index}"),
            replacement: Value::Null,
        });
    }
    masks
}
