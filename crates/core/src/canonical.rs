// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{Error, Result, SubjectId};
use serde::Serialize;

/// Canonical digest for a normalized durable semantic request.
pub fn canonical_request_digest<T: Serialize>(
    operation_kind: &str,
    subject: SubjectId,
    request: &T,
) -> Result<String> {
    let value = serde_json::to_value(request).map_err(|error| {
        Error::new(
            crate::ErrorOwner::Core,
            crate::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(error)
    })?;
    let canonical = canonical_json(&value)?;
    let input = format!(
        "{operation_kind}\n{}\n{canonical}",
        subject.0.to_string().to_lowercase()
    );
    Ok(blake3::hash(input.as_bytes()).to_hex().to_string())
}

fn canonical_json(value: &serde_json::Value) -> Result<String> {
    match value {
        serde_json::Value::Null => Ok("null".into()),
        serde_json::Value::Bool(value) => Ok(value.to_string()),
        serde_json::Value::Number(value) => Ok(value.to_string()),
        serde_json::Value::String(value) => serde_json::to_string(value).map_err(|error| {
            Error::new(
                crate::ErrorOwner::Core,
                crate::ErrorCode::InvalidRequest,
                "Invalid request",
            )
            .with_source(error)
        }),
        serde_json::Value::Array(values) => values
            .iter()
            .map(canonical_json)
            .collect::<Result<Vec<_>>>()
            .map(|values| format!("[{}]", values.join(","))),
        serde_json::Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort();
            let mut pairs = Vec::with_capacity(keys.len());
            for key in keys {
                pairs.push(format!(
                    "{}:{}",
                    serde_json::to_string(key).map_err(|error| Error::new(
                        crate::ErrorOwner::Core,
                        crate::ErrorCode::InvalidRequest,
                        "Invalid request"
                    )
                    .with_source(error))?,
                    canonical_json(&values[key])?
                ));
            }
            Ok(format!("{{{}}}", pairs.join(",")))
        }
    }
}
