// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use nous_core::*;
use nous_runtime::{ContextResolver, ReadContext, bounded_text};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ContextOwner {
    Cognition,
    Material,
    Resource,
    Unsupported,
}

fn context_owner(reference: &CognitiveRef) -> ContextOwner {
    match reference {
        CognitiveRef::Memory(_)
        | CognitiveRef::MemoryRevision(_)
        | CognitiveRef::Episode(_)
        | CognitiveRef::EpisodeRevision(_)
        | CognitiveRef::Journal(_)
        | CognitiveRef::JournalRevision(_)
        | CognitiveRef::CognitiveSchema(_)
        | CognitiveRef::CognitiveSchemaRevision(_) => ContextOwner::Cognition,
        CognitiveRef::Artifact(_)
        | CognitiveRef::Occurrence(_)
        | CognitiveRef::SourceRegion(_)
        | CognitiveRef::DerivedRepresentation(_)
        | CognitiveRef::DerivedRegion(_) => ContextOwner::Material,
        CognitiveRef::Resource(_) => ContextOwner::Resource,
        _ => ContextOwner::Unsupported,
    }
}

#[async_trait::async_trait]
impl ContextResolver for NousRuntime {
    async fn context_sources(
        &self,
        context: &ReadContext,
        references: &[CognitiveRef],
        explicit_references: &HashSet<CognitiveRef>,
        max_items: usize,
        max_text_bytes: usize,
    ) -> Result<(Vec<ContextContribution>, Vec<Degradation>)> {
        // Select once in caller order. Each semantic owner receives one
        // grouped pass (split only at its 256-reference read bound), even when
        // references for different owners are interleaved.
        let mut seen = HashSet::new();
        let references = references
            .iter()
            .filter(|reference| seen.insert((*reference).clone()))
            .take(max_items.min(2048))
            .cloned()
            .collect::<Vec<_>>();
        let resource_refs = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::Resource(resource) => Some(resource.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let descriptors = if resource_refs.is_empty() {
            HashMap::new()
        } else {
            self.cognition
                .list_resources(context.subject)
                .await?
                .into_iter()
                .filter(|view| resource_refs.contains(&view.descriptor.resource_ref))
                .map(|view| {
                    let descriptor = view.descriptor;
                    (descriptor.resource_ref.clone(), descriptor)
                })
                .collect::<HashMap<_, _>>()
        };

        let mut owner_references = HashMap::<ContextOwner, Vec<CognitiveRef>>::new();
        for reference in &references {
            owner_references
                .entry(context_owner(reference))
                .or_default()
                .push(reference.clone());
        }
        let mut by_reference = HashMap::new();
        let mut owner_degradation = HashMap::<String, Vec<Degradation>>::new();
        let mut general_degradation = Vec::new();
        for owner in [
            ContextOwner::Cognition,
            ContextOwner::Material,
            ContextOwner::Resource,
            ContextOwner::Unsupported,
        ] {
            let Some(group) = owner_references.remove(&owner) else {
                continue;
            };
            for batch in group.chunks(256) {
                match owner {
                    ContextOwner::Cognition => {
                        if let Some(memory) = &self.memory {
                            let (views, batch_degradation) = memory
                                .context_contributions(
                                    context,
                                    batch,
                                    explicit_references,
                                    batch.len(),
                                    max_text_bytes,
                                )
                                .await?;
                            insert_contributions(&mut by_reference, views);
                            collect_degradation(
                                &mut owner_degradation,
                                &mut general_degradation,
                                batch_degradation,
                            );
                        } else {
                            for reference in batch {
                                owner_degradation
                                    .entry(reference.to_string())
                                    .or_default()
                                    .push(unavailable_source(reference));
                            }
                        }
                    }
                    ContextOwner::Material => {
                        let (views, batch_degradation) = self
                            .material
                            .context_contributions(
                                context,
                                batch,
                                explicit_references,
                                batch.len(),
                                max_text_bytes,
                            )
                            .await?;
                        insert_contributions(&mut by_reference, views);
                        collect_degradation(
                            &mut owner_degradation,
                            &mut general_degradation,
                            batch_degradation,
                        );
                    }
                    ContextOwner::Resource => {
                        for reference in batch {
                            let Some(resource) = (match reference {
                                CognitiveRef::Resource(resource) => Some(resource),
                                _ => None,
                            }) else {
                                continue;
                            };
                            let Some(descriptor) = descriptors.get(resource) else {
                                owner_degradation
                                    .entry(reference.to_string())
                                    .or_default()
                                    .push(unavailable_source(reference));
                                continue;
                            };
                            let text = descriptor
                                .display_label
                                .clone()
                                .unwrap_or_else(|| resource.as_str().to_owned());
                            let text = bounded_text(text, max_text_bytes);
                            by_reference.insert(
                                reference.clone(),
                                ContextContribution {
                                    source_revision: None,
                                    reference: reference.clone(),
                                    semantic_role: "resource_awareness".into(),
                                    estimated_tokens: Some(
                                        text.len().div_ceil(4).min(u32::MAX as usize) as u32,
                                    ),
                                    text: Some(text),
                                    authority: AuthorityClass::ResourceDescriptor,
                                    freshness: FreshnessDescriptor {
                                        occurred: Vec::new(),
                                        observed_at: None,
                                        valid_time: TemporalExtent::Unknown,
                                        formed_at: None,
                                        recorded_at: None,
                                    },
                                    evidence: Vec::new(),
                                    provenance: ProvenanceSummary {
                                        source_count: 0,
                                        producer_signatures: Vec::new(),
                                        note: Some("resource awareness".into()),
                                    },
                                    priority: 1.0,
                                    materialization: None,
                                },
                            );
                        }
                    }
                    ContextOwner::Unsupported => {}
                }
            }
        }

        let mut remaining_text_bytes = max_text_bytes;
        let mut ordered = Vec::with_capacity(references.len());
        let mut degradation = Vec::new();
        for reference in &references {
            let key = reference.to_string();
            if let Some(mut contribution) = by_reference.remove(reference) {
                if let Some(text) = contribution.text.take() {
                    let bounded = bounded_text(text, remaining_text_bytes);
                    remaining_text_bytes = remaining_text_bytes.saturating_sub(bounded.len());
                    contribution.estimated_tokens =
                        Some(((bounded.len().saturating_add(3)) / 4).min(u32::MAX as usize) as u32);
                    contribution.text = Some(bounded);
                }
                ordered.push(contribution);
                if let Some(values) = owner_degradation.remove(&key) {
                    degradation.extend(values);
                }
            } else if let Some(values) = owner_degradation.remove(&key) {
                degradation.extend(values);
            } else {
                degradation.push(unavailable_source(reference));
            }
        }
        degradation.extend(owner_degradation.into_values().flatten());
        degradation.extend(general_degradation);
        Ok((ordered, degradation))
    }
}

fn insert_contributions(
    target: &mut HashMap<CognitiveRef, ContextContribution>,
    contributions: Vec<ContextContribution>,
) {
    for contribution in contributions {
        target.insert(contribution.reference.clone(), contribution);
    }
}

fn collect_degradation(
    by_reference: &mut HashMap<String, Vec<Degradation>>,
    general: &mut Vec<Degradation>,
    degradation: Vec<Degradation>,
) {
    for value in degradation {
        if let Some(reference) = value.detail.clone() {
            by_reference.entry(reference).or_default().push(value);
        } else {
            general.push(value);
        }
    }
}

fn unavailable_source(reference: &CognitiveRef) -> Degradation {
    Degradation {
        code: "projection_source_unavailable".into(),
        detail: Some(reference.to_string()),
    }
}
