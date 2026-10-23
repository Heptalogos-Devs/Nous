// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0 AND AGPL-3.0-only
// Adapted sources and licenses: ../structured-sources.json.

use super::MAX_CHANGES;
use nous_domain as d;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCatalogEntry {
    pub key: String,
    pub source: d::EvidenceSource,
    pub text: String,
    #[serde(default)]
    pub source_event_kind: Option<d::ObservationKind>,
    #[serde(default)]
    pub truncated: bool,
}
impl SourceCatalogEntry {
    pub fn new(key: impl Into<String>, source: d::EvidenceSource, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            source,
            text: text.into(),
            source_event_kind: None,
            truncated: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredFormationInput {
    pub namespace_id: d::NamespaceId,
    pub subject_id: d::SubjectId,
    pub knowledge_cutoff: i64,
    pub sources: Vec<SourceCatalogEntry>,
    pub slots: Vec<d::StructuredTargetSlot>,
}
impl StructuredFormationInput {
    pub fn validate(&self) -> d::Result<()> {
        if self.sources.is_empty()
            || self.sources.len() > 128
            || self.slots.is_empty()
            || self.slots.len() > MAX_CHANGES
            || self.knowledge_cutoff < 0
        {
            return Err(d::Error::invalid(
                "structured input requires 1..128 sources, 1..32 target slots and a nonnegative knowledge cutoff",
            ));
        }
        let mut keys = BTreeSet::new();
        for source in &self.sources {
            validate_key(&source.key)?;
            source.source.validate()?;
            if source.text.trim().is_empty() || !keys.insert(source.key.as_str()) {
                return Err(d::Error::invalid(
                    "source catalog text must be nonempty and keys unique",
                ));
            }
        }
        let mut identities = BTreeSet::new();
        for slot in &self.slots {
            validate_key(&slot.key)?;
            if slot.kind == d::CognitiveKind::Memory
                || slot.expected_revision < 0
                || slot.expected_revision == i64::MAX
                || !keys.insert(slot.key.as_str())
                || !identities.insert((slot.kind, slot.id.as_str()))
            {
                return Err(d::Error::invalid(
                    "structured slots require unique supported targets and keys with nonnegative expected revisions",
                ));
            }
            slot.output_reference()?;
            if slot.kind == d::CognitiveKind::MemorySemantics
                && !self
                    .sources
                    .iter()
                    .any(|entry| matches!(entry.source, d::EvidenceSource::MemoryRevision { .. }))
            {
                return Err(d::Error::invalid(
                    "memory semantics requires an exact existing Memory in the source catalog",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn reference(&self, key: &str) -> d::Result<d::CognitiveRef> {
        if let Some(entry) = self.sources.iter().find(|entry| entry.key == key) {
            return entry.source.cognitive_ref().ok_or_else(|| {
                d::Error::invalid(format!(
                    "source key {key} is evidence, not a cognition reference"
                ))
            });
        }
        self.slots
            .iter()
            .find(|slot| slot.key == key)
            .ok_or_else(|| d::Error::invalid(format!("unknown cognition catalog/slot key: {key}")))?
            .output_reference()
    }

    pub(super) fn evidence_source(&self, key: &str) -> d::Result<d::EvidenceSource> {
        if let Some(entry) = self.sources.iter().find(|entry| entry.key == key) {
            return Ok(entry.source.clone());
        }
        self.reference(key)
            .map(|reference| d::EvidenceSource::CognitiveRevision { reference })
    }

    pub(super) fn observation(&self, key: &str) -> d::Result<d::ObservationId> {
        match self.evidence_source(key)? {
            d::EvidenceSource::Observation { observation_id } => Ok(observation_id),
            _ => Err(d::Error::invalid(
                "grounding observation must select an Observation source catalog key",
            )),
        }
    }
}
pub(super) fn validate_key(key: &str) -> d::Result<()> {
    if key.is_empty()
        || key.len() > 256
        || key
            .chars()
            .any(|value| value.is_control() || value.is_whitespace())
    {
        return Err(d::Error::invalid(
            "catalog, slot and group keys require 1..256 visible non-whitespace bytes",
        ));
    }
    Ok(())
}
