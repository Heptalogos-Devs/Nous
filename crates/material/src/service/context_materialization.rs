//! Convert owner-visible material into bounded ReadContext contributions.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::materialization::is_text_media;
use super::*;

impl MaterialService {
    pub async fn context_contributions(
        &self,
        context: &nous_runtime::ReadContext,
        references: &[CognitiveRef],
        _explicit_references: &std::collections::HashSet<CognitiveRef>,
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)> {
        let mut seen = std::collections::HashSet::new();
        let selected = references
            .iter()
            .filter(|reference| {
                matches!(
                    reference,
                    CognitiveRef::Artifact(_)
                        | CognitiveRef::Occurrence(_)
                        | CognitiveRef::SourceRegion(_)
                        | CognitiveRef::DerivedRepresentation(_)
                        | CognitiveRef::DerivedRegion(_)
                )
            })
            .filter(|reference| seen.insert((*reference).clone()))
            .take(max_items.min(256))
            .cloned()
            .collect::<Vec<_>>();
        if selected.is_empty() || max_text_bytes == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let materialized = self
            .materialize_batch(
                context.subject,
                &selected,
                (max_text_bytes as u64).min(self.max_upload_bytes),
                context.historical_view(),
            )
            .await?;
        let mut contributions = Vec::new();
        let mut degradation = Vec::new();
        for reference in selected {
            let Some(material) = materialized.get(&reference) else {
                degradation.push(Degradation {
                    code: "projection_source_unavailable".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            };
            if !is_text_media(&material.media_type) {
                degradation.push(Degradation {
                    code: "projection_source_not_text".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            }
            let Ok(text) = std::str::from_utf8(&material.bytes) else {
                degradation.push(Degradation {
                    code: "projection_source_unavailable".into(),
                    detail: Some(reference.to_string()),
                });
                continue;
            };
            let evidence = material
                .provenance
                .iter()
                .filter(|source| *source != &reference)
                .cloned()
                .map(|source| EvidenceHandle {
                    epistemic_relation: None,
                    reference: source,
                    basis_role: "source".into(),
                })
                .collect::<Vec<_>>();
            let producer_signatures = material
                .producer
                .as_ref()
                .and_then(|producer| producer.get("signature_hash"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .into_iter()
                .collect();
            let authority = if material.producer.is_some() {
                AuthorityClass::Interpretation
            } else {
                AuthorityClass::Evidence
            };
            contributions.push(ContextContribution {
                source_revision: Some(material.reference.clone()),
                reference: reference.clone(),
                semantic_role: if material.producer.is_some() {
                    "derived_interpretation".into()
                } else {
                    "material_evidence".into()
                },
                text: Some(text.to_owned()),
                authority,
                freshness: FreshnessDescriptor {
                    occurred: Vec::new(),
                    observed_at: None,
                    valid_time: TemporalExtent::Unknown,
                    formed_at: None,
                    recorded_at: None,
                },
                evidence,
                provenance: ProvenanceSummary {
                    source_count: material.provenance.len().saturating_sub(1),
                    producer_signatures,
                    note: None,
                },
                priority: 1.0,
                estimated_tokens: Some(
                    ((text.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32
                ),
                materialization: Some(MaterializationHandle {
                    reference: material.reference.clone(),
                    level: "text_excerpt".into(),
                }),
            });
        }
        Ok((contributions, degradation))
    }
}
