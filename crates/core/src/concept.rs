//! Shared Tag semantic contract; identity and aliases never substitute for meaning.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
pub const TAG_REPRESENTATION_VERSION: &str = "tag-semantic-representation-v1";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSemanticRepresentation {
    pub version: String,
    pub text: String,
    pub digest: String,
}
pub fn tag_semantic_representation(
    label: &str,
    description: Option<&str>,
    kind: Option<&str>,
) -> Result<TagSemanticRepresentation> {
    if label.trim().is_empty()
        || label.len() > 256
        || description.is_some_and(|v| v.len() > 4096)
        || kind.is_some_and(|v| v.len() > 128)
    {
        return Err(Error::new(
            crate::ErrorOwner::Concept,
            crate::ErrorCode::InvalidRequest,
            "invalid Tag content bounds",
        ));
    }
    let normalize = |value: &str| {
        value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .trim()
            .to_owned()
    };
    let mut sections = vec![format!("Concept:\n{}", normalize(label))];
    for (heading, value) in [("Description", description), ("Kind", kind)] {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            sections.push(format!("{heading}:\n{}", normalize(value)));
        }
    }
    let text = sections.join("\n\n");
    let digest = blake3::hash(format!("{TAG_REPRESENTATION_VERSION}\n{text}").as_bytes())
        .to_hex()
        .to_string();
    Ok(TagSemanticRepresentation {
        version: TAG_REPRESENTATION_VERSION.into(),
        text,
        digest,
    })
}
