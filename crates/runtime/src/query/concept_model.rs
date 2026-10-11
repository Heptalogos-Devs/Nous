//! Query-local model contract: catalog keys resolve only against the frozen Serving view.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::{
    ActivationSeed, ActivationSource, NovelConceptHypothesis, QueryActivation, TagActivation,
};
use nous_core::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryConceptCandidate {
    pub key: String,
    pub tag: TagId,
    pub semantic_text: String,
    pub strength: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryConceptSelection {
    pub key: String,
    pub strength: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryConceptOutput {
    pub existing_tags: Vec<QueryConceptSelection>,
    pub novel_concepts: Vec<NovelConceptHypothesis>,
}
impl QueryActivation {
    pub fn apply_concept_model(
        &mut self,
        output: QueryConceptOutput,
        policy: &super::ConceptActivationPolicy,
    ) -> Result<()> {
        if output.existing_tags.len() > 8 || output.novel_concepts.len() > 4 {
            return Err(Error::Invalid("query concept output exceeds bounds".into()));
        }
        let mut selected = Vec::new();
        let mut keys = std::collections::HashSet::new();
        for selection in output.existing_tags {
            if !keys.insert(selection.key.clone())
                || !selection.strength.is_finite()
                || !(0.0..=1.0).contains(&selection.strength)
            {
                return Err(Error::Invalid(
                    "invalid query concept key or strength".into(),
                ));
            }
            let candidate = self
                .concept_catalog
                .iter()
                .find(|entry| entry.key == selection.key)
                .ok_or_else(|| Error::Invalid("query concept key outside frozen catalog".into()))?;
            selected.push(TagActivation {
                tag: candidate.tag,
                strength: selection.strength,
                source: ActivationSource::ModelInferred,
                origin: "model_inferred".into(),
            });
        }
        if output.novel_concepts.iter().any(|hypothesis| {
            hypothesis.text.trim().is_empty() || hypothesis.text.chars().count() > 512
        }) {
            return Err(Error::Invalid("invalid query concept hypothesis".into()));
        }
        for tag in selected {
            if let Some(existing) = self
                .inferred_tags
                .iter_mut()
                .find(|existing| existing.tag == tag.tag)
            {
                if tag.strength >= existing.strength {
                    *existing = tag;
                }
            } else if !self
                .explicit_tags
                .iter()
                .any(|explicit| explicit.tag == tag.tag)
            {
                self.inferred_tags.push(tag);
            }
        }
        self.inferred_tags.sort_by(|a, b| {
            b.strength
                .total_cmp(&a.strength)
                .then(a.tag.0.cmp(&b.tag.0))
        });
        self.inferred_tags.truncate(policy.max_activated_tags);
        self.seeds.retain(|seed| {
            !matches!(
                seed.origin.as_str(),
                "existing_semantic_match" | "model_inferred"
            )
        });
        self.seeds
            .extend(self.inferred_tags.iter().map(|tag| ActivationSeed {
                reference: CognitiveRef::Tag(tag.tag),
                origin: tag.origin.clone(),
                strength: tag.strength,
            }));
        self.novel_concepts = output.novel_concepts;
        self.model_calls = 1;
        self.model_completed = true;
        Ok(())
    }
}
