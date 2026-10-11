// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::ReadContext;
use nous_core::{CognitiveRef, ContextContribution, Degradation, Modality, Result};
use std::collections::HashSet;

/// Batch dispatch for the semantic owners that materialize a consumer context.
/// Runtime defines the shared read facts; Memory and Material own their data.
#[async_trait::async_trait]
pub trait ContextResolver: Send + Sync {
    async fn context_sources(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
        explicit_references: &HashSet<CognitiveRef>,
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)>;
}

pub fn modality(media: &str) -> Modality {
    if media.starts_with("text/") {
        Modality::Text
    } else if media.starts_with("image/") {
        Modality::Image
    } else if media.starts_with("audio/") {
        Modality::Audio
    } else if media.starts_with("video/") {
        Modality::Video
    } else if media.contains("json") || media.contains("xml") {
        Modality::Structured
    } else {
        Modality::Binary
    }
}

pub fn bounded_text(mut text: String, max_bytes: usize) -> String {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text
}
