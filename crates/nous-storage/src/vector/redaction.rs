//! Erasing unowned content preserves actual input identities and unrelated batch results.
use crate::{CallDocument, CallRedaction, WriteTransaction};
use nous_domain::*;
use serde_json::{Value, json};
impl WriteTransaction {
    pub(super) async fn redact_embedding_hash(
        &mut self,
        call: &ModelCall,
        hash: &str,
    ) -> Result<()> {
        let Some(inputs) = call
            .request
            .pointer("/input/inputs")
            .and_then(Value::as_array)
        else {
            return Ok(());
        };
        let mut masks = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            if super::call_content::input_hash(input)? != hash {
                continue;
            }
            let tombstone = if input.get("redacted") == Some(&Value::Bool(true)) {
                input.clone()
            } else {
                let actual: EmbeddingInputItem = serde_json::from_value(input.clone())?;
                let sources = actual
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        EmbeddingPart::Text { source, .. } => source.as_ref(),
                        EmbeddingPart::Media { source, .. } => Some(source),
                    })
                    .collect::<Vec<_>>();
                let blobs = actual
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        EmbeddingPart::Media { blob, .. } => Some(blob),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                json!({"target":actual.target,"source_hash":hash,"source_refs":sources,"blob_refs":blobs,"redacted":true,"reason":"no_remaining_content_consumers"})
            };
            for (document, pointer, replacement) in [
                (
                    CallDocument::Request,
                    format!("/input/inputs/{index}"),
                    tombstone.clone(),
                ),
                (
                    CallDocument::Request,
                    format!("/preview/input/inputs/{index}"),
                    json!([]),
                ),
                (
                    CallDocument::Response,
                    format!("/record/metadata/input/inputs/{index}"),
                    tombstone,
                ),
                (
                    CallDocument::Response,
                    format!("/record/metadata/preview/input/inputs/{index}"),
                    json!([]),
                ),
                (
                    CallDocument::Response,
                    format!("/result/{index}"),
                    Value::Null,
                ),
            ] {
                masks.push(CallRedaction {
                    document,
                    pointer,
                    replacement,
                });
            }
        }
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
        self.redact_model_call(&call.id, &masks, "embedding_content_purged")
            .await
    }
}
