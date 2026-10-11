//! Direct concept recall and optional existing-Tag semantic activation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_runtime::{
    ActivationSeed, ActivationSource, BoundQuery, ConceptEnrichment, LaneCandidate, LaneOutput,
    LaneStatus, QueryActivation, QueryPlan, QuerySemanticEmbedding, TagActivation,
};
pub(crate) async fn activate(
    snapshot: &ServingSnapshot,
    bound: &BoundQuery,
    provider: Option<&dyn TextEmbeddingProvider>,
) -> Result<QueryActivation> {
    if bound
        .historical_authority
        .as_ref()
        .is_some_and(|view| snapshot.view_digest.as_ref() != Some(&view.snapshot_digest))
    {
        return Err(Error::Unavailable(
            "query activation requires a compatible historical Serving view".into(),
        ));
    }
    let mut activation = bound.activation.clone();
    if activation.frozen {
        return Ok(activation);
    }
    if bound.concept_enrichment == ConceptEnrichment::Off {
        return Ok(activation);
    }
    let policy = nous_runtime::ConceptActivationPolicy::from_snapshot(&bound.config_snapshot)?;
    if let Some(generation) = snapshot
        .concept
        .iter()
        .max_by_key(|generation| generation.authority_watermark)
    {
        activation.concept_catalog = model_catalog(generation, &[], policy.model_catalog_limit);
    }
    let generation = snapshot.concept.iter().find(|g| {
        g.space
            .as_ref()
            .is_some_and(|space| provider.is_some_and(|p| p.space().compatible_with(space)))
    });
    let failure =
        if bound.source_query.capabilities.text_embedding == RequirementStrength::Forbidden {
            Some("query embedding is forbidden".into())
        } else if let (Some(provider), Some(generation)) = (provider, generation) {
            match provider
                .embed(TextEmbeddingRequest {
                    subject: bound.source_query.subject,
                    text: bound.representation.text.clone(),
                    query: true,
                })
                .await
            {
                Ok(embedding) => {
                    if !generation
                        .space
                        .as_ref()
                        .is_some_and(|space| embedding.space.compatible_with(space))
                        || !provider.producers().iter().any(|producer| {
                            producer.signature_hash == embedding.producer.signature_hash
                        })
                    {
                        Some("query/concept embedding space or producer mismatch".into())
                    } else {
                        let mut matches = semantic_matches(generation, &embedding.vector, &policy);
                        matches.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.0.cmp(&b.0.0)));
                        let explicit = activation
                            .explicit_tags
                            .iter()
                            .map(|a| a.tag)
                            .collect::<std::collections::HashSet<_>>();
                        activation.inferred_tags = matches
                            .into_iter()
                            .filter(|(tag, _)| !explicit.contains(tag))
                            .take(policy.max_activated_tags)
                            .map(|(tag, strength)| TagActivation {
                                tag,
                                strength,
                                source: ActivationSource::ExistingSemanticMatch,
                                origin: "existing_semantic_match".into(),
                            })
                            .collect();
                        activation
                            .seeds
                            .extend(activation.inferred_tags.iter().map(|tag| ActivationSeed {
                                reference: CognitiveRef::Tag(tag.tag),
                                origin: tag.origin.clone(),
                                strength: tag.strength,
                            }));
                        activation.concept_catalog = model_catalog(
                            generation,
                            &activation.inferred_tags,
                            policy.model_catalog_limit,
                        );
                        activation.concept_generation = Some(generation.generation_id);
                        activation.query_embedding_digest =
                            Some(crate::assets::files::digest(&embedding)?);
                        activation.embedding = Some(QuerySemanticEmbedding {
                            vector: embedding.vector,
                            space: embedding.space,
                            producer: embedding.producer,
                        });
                        None
                    }
                }
                Err(error) => Some(error.to_string()),
            }
        } else {
            Some("shared concept vectors or query embedding provider unavailable".into())
        };
    if let Some(detail) = failure {
        activation.degradation.push(Degradation {
            code: "concept_enrichment_unavailable".into(),
            detail: Some(detail),
        });
    }
    Ok(activation)
}
pub(crate) fn direct_lane(
    snapshot: &ServingSnapshot,
    bound: &BoundQuery,
    plan: &QueryPlan,
) -> LaneOutput {
    let mut output = LaneOutput::empty(EvidenceFamily::TagDirect, LaneStatus::Ready);
    let Some(generation) = snapshot
        .concept
        .iter()
        .max_by_key(|g| g.authority_watermark)
    else {
        output.status = LaneStatus::Unavailable;
        output
            .diagnostics
            .push("concept serving generation unavailable".into());
        return output;
    };
    output.generation_ref = Some(generation.generation_id);
    output.authority_watermark = Some(generation.authority_watermark);
    let mut candidates =
        std::collections::HashMap::<CognitiveRef, (f64, Vec<serde_json::Value>)>::new();
    for activation in bound.activation.tags() {
        if let Some(record) = generation.record(activation.tag) {
            for reference in &record.attachments {
                if !bound.source_query.projection.allows_reference(reference) {
                    continue;
                }
                let candidate = candidates
                    .entry(reference.clone())
                    .or_insert((0.0, Vec::new()));
                candidate.0 = candidate.0.max(activation.strength);
                candidate.1.push(serde_json::json!({"tag":activation.tag,"source":activation.source,"strength":activation.strength}));
            }
        }
    }
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.1.0
            .total_cmp(&a.1.0)
            .then_with(|| a.0.to_string().cmp(&b.0.to_string()))
    });
    if candidates.len() > plan.lane_budget(EvidenceFamily::TagDirect) {
        output.status = LaneStatus::Truncated;
    }
    output.candidates=candidates.into_iter().take(plan.lane_budget(EvidenceFamily::TagDirect)).enumerate().map(|(i,(reference,(_,activations)))|LaneCandidate {reference,rank:(i+1) as u32,variants:vec!["concept:direct_attachment".into()],provider_metadata:serde_json::json!({"activations":activations,"concept_generation":generation.generation_id})}).collect();
    output
}

fn semantic_matches(
    generation: &ConceptGeneration,
    query: &[f32],
    policy: &nous_runtime::ConceptActivationPolicy,
) -> Vec<(TagId, f64)> {
    generation
        .records
        .iter()
        .filter_map(|record| {
            let vector = record.vector.as_ref()?;
            if vector.len() != query.len() {
                return None;
            }
            let dot = vector
                .iter()
                .zip(query)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            let norm = vector
                .iter()
                .map(|a| f64::from(*a).powi(2))
                .sum::<f64>()
                .sqrt()
                * query
                    .iter()
                    .map(|b| f64::from(*b).powi(2))
                    .sum::<f64>()
                    .sqrt();
            let similarity = dot / norm;
            (similarity.is_finite() && similarity >= policy.minimum_similarity)
                .then_some((record.tag, similarity.clamp(0.0, 1.0)))
        })
        .collect()
}

fn model_catalog(
    generation: &ConceptGeneration,
    inferred: &[TagActivation],
    limit: usize,
) -> Vec<nous_runtime::QueryConceptCandidate> {
    let mut candidates = generation
        .records
        .iter()
        .map(|record| {
            (
                record,
                inferred
                    .iter()
                    .find(|tag| tag.tag == record.tag)
                    .map_or(0.0, |tag| tag.strength),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.tag.0.cmp(&b.0.tag.0)));
    candidates
        .into_iter()
        .take(limit)
        .enumerate()
        .map(
            |(i, (record, strength))| nous_runtime::QueryConceptCandidate {
                key: format!("c{i}"),
                tag: record.tag,
                semantic_text: record.semantic.text.clone(),
                strength,
            },
        )
        .collect()
}
