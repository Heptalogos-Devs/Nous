//! Persistent payload masks remain authoritative when an external attempt completes late.
use crate::{WriteTransaction, codec::*, storage_error, write_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallDocument {
    Request,
    Response,
    Error,
}
#[derive(Debug, Clone)]
pub struct CallRedaction {
    pub document: CallDocument,
    pub pointer: String,
    pub replacement: Value,
}
impl WriteTransaction {
    pub async fn redact_model_call(
        &mut self,
        id: &ModelCallId,
        masks: &[CallRedaction],
        reason: &str,
    ) -> Result<()> {
        let current = self.model_call(id).await?;
        self.context()
            .require_subject(&current.namespace_id, &current.subject_id)?;
        for mask in masks {
            if (!mask.pointer.is_empty() && !mask.pointer.starts_with('/'))
                || (matches!(mask.document, CallDocument::Error) && !mask.pointer.is_empty())
            {
                return Err(Error::invalid("invalid model payload redaction pointer"));
            }
            sqlx::query("INSERT INTO model_call_redactions(call_id,document,pointer,replacement,reason,commit_seq) VALUES(?,?,?,?,?,?) ON CONFLICT(call_id,document,pointer) DO UPDATE SET replacement=excluded.replacement,reason=excluded.reason,commit_seq=excluded.commit_seq")
                .bind(id.as_str()).bind(enum_name(&mask.document)?).bind(&mask.pointer).bind(serde_json::to_string(&mask.replacement)?).bind(reason).bind(self.commit_seq()).execute(&mut *self.tx).await.map_err(write_error)?;
        }
        let mut current = self.model_call(id).await?;
        let original = current.clone();
        self.apply_call_masks(&mut current).await?;
        if fully_redacted(&current) {
            preserve_accounting(&mut current, &original);
        }
        sqlx::query("UPDATE model_calls SET request=?,response=?,error=? WHERE id=?")
            .bind(serde_json::to_string(&current.request)?)
            .bind(json_option(&current.response)?)
            .bind(json_option(&current.error)?)
            .bind(id.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        Ok(())
    }
    pub async fn merge_model_call_completion(
        &mut self,
        completed: &ModelCall,
    ) -> Result<ModelCall> {
        let current = self.model_call(&completed.id).await?;
        self.context()
            .require_subject(&current.namespace_id, &current.subject_id)?;
        let mut merged = completed.clone();
        self.apply_call_masks(&mut merged).await?;
        if merged.request != current.request {
            return Err(Error::new(
                ErrorCode::Conflict,
                "model request differs from its immutable reservation and redaction masks",
            ));
        }
        if fully_redacted(&merged) {
            preserve_accounting(&mut merged, completed);
        }
        self.update_model_call(&merged).await?;
        Ok(merged)
    }
    async fn apply_call_masks(&mut self, call: &mut ModelCall) -> Result<()> {
        let masks=sqlx::query("SELECT document,pointer,replacement FROM model_call_redactions WHERE call_id=? ORDER BY document,length(pointer),pointer")
            .bind(call.id.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        for row in masks {
            let document: CallDocument = parse_enum(row.get("document"))?;
            let pointer: String = row.get("pointer");
            let replacement: Value = serde_json::from_str(row.get("replacement"))?;
            match document {
                CallDocument::Request => apply(&mut call.request, &pointer, &replacement),
                CallDocument::Response => {
                    let mut response = call.response.take().unwrap_or(Value::Null);
                    apply(&mut response, &pointer, &replacement);
                    call.response = (!response.is_null()).then_some(response);
                }
                CallDocument::Error => {
                    if let Some(error) = &call.error {
                        let mut redacted = Error::new(
                            error.code,
                            replacement
                                .as_str()
                                .unwrap_or("model error detail was permanently redacted"),
                        );
                        redacted.trace_id = error.trace_id.clone();
                        call.error = Some(redacted);
                    }
                }
            }
        }
        Ok(())
    }
}
fn fully_redacted(call: &ModelCall) -> bool {
    call.request.get("redacted") == Some(&Value::Bool(true))
        || call
            .response
            .as_ref()
            .is_some_and(|value| value.get("redacted") == Some(&Value::Bool(true)))
}
fn apply(document: &mut Value, pointer: &str, replacement: &Value) {
    if pointer.is_empty() {
        *document = replacement.clone();
    } else if let Some(field) = document.pointer_mut(pointer) {
        *field = replacement.clone();
    }
}
fn preserve_accounting(masked: &mut ModelCall, completed: &ModelCall) {
    let Some(actual) = completed
        .response
        .as_ref()
        .and_then(|response| response.get("record"))
    else {
        return;
    };
    let response = masked
        .response
        .get_or_insert_with(|| json!({"redacted":true}));
    if !response.is_object() {
        *response = json!({"redacted":true});
    }
    if response
        .get("record")
        .is_none_or(|record| !record.is_object())
    {
        response["record"] = json!({});
    }
    for key in [
        "provider_model",
        "usage",
        "cost",
        "started_at",
        "completed_at",
        "status",
    ] {
        if let Some(value) = actual.get(key) {
            response["record"][key] = value.clone();
        }
    }
}
