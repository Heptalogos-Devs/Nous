// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::topology_lane::topology_lane;
use crate::*;
use nous_core::{CognitiveRef, Cue, EvidenceFamily, RequirementStrength, Result};
use nous_runtime::{
    BoundQuery, LaneCandidate, LaneOutput, LaneStatus, QueryPlan, SharedLaneProvider,
};
use std::collections::HashMap;

fn text_query(query: &nous_core::CognitiveQuery) -> String {
    query
        .expression
        .cues
        .iter()
        .filter_map(|cue| match cue {
            Cue::Text(value) => Some(value.text.as_str()),
            Cue::Example(value) => Some(value.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Signals prepared once for one bound leaf. Consumers only borrow this value;
/// consuming it for Runtime lanes happens after cognitive readout.
pub struct PreparedQuerySignals {
    query_text: String,
    embedding: Option<TextEmbeddingOutput>,
    lexical: Option<LaneOutput>,
    dense: Option<LaneOutput>,
}

impl PreparedQuerySignals {
    pub fn query_text(&self) -> &str {
        &self.query_text
    }
    pub fn embedding(&self) -> Option<&TextEmbeddingOutput> {
        self.embedding.as_ref()
    }
    pub fn lexical(&self) -> Option<&LaneOutput> {
        self.lexical.as_ref()
    }
    pub fn dense(&self) -> Option<&LaneOutput> {
        self.dense.as_ref()
    }
    fn into_lanes(self) -> Vec<LaneOutput> {
        self.lexical.into_iter().chain(self.dense).collect()
    }
}

fn prepare_lexical(
    snapshot: &ServingSnapshot,
    query: &nous_core::CognitiveQuery,
    query_text: &str,
    plan: &QueryPlan,
) -> Result<LaneOutput> {
    let lexical = {
        let mut output = LaneOutput::empty(EvidenceFamily::Lexical, LaneStatus::Unavailable);
        if query_text.trim().is_empty() {
            output.status = LaneStatus::Ready;
        } else if let Some(index) = snapshot.lexical.as_ref() {
            output.status = LaneStatus::Ready;
            output.generation_ref = Some(index.generation_id);
            output.candidates = index
                .search_with_domains(
                    query_text,
                    plan.lane_budget(EvidenceFamily::Lexical),
                    &query.projection.domain_names(),
                )?
                .into_iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    item.reference.map(|reference| LaneCandidate {
                        reference,
                        rank: (index + 1) as u32,
                        variants: vec!["lexical:generation".into()],
                        provider_metadata: serde_json::Value::Null,
                    })
                })
                .collect();
        } else {
            output
                .diagnostics
                .push("lexical serving generation is unavailable".into());
        }
        output
    };

    Ok(lexical)
}

fn prepare_dense(
    snapshot: &ServingSnapshot,
    query: &nous_core::CognitiveQuery,
    query_text: &str,
    plan: &QueryPlan,
    embedding: Option<&TextEmbeddingOutput>,
    embedding_error: Option<String>,
    provider_available: bool,
) -> Result<LaneOutput> {
    let mut output = LaneOutput::empty(EvidenceFamily::Dense, LaneStatus::Unavailable);
    if query_text.trim().is_empty()
        || query.capabilities.text_embedding == RequirementStrength::Forbidden
    {
        output.status = LaneStatus::Ready;
    } else if let Some(embedding) = embedding {
        let mut best = HashMap::<CognitiveRef, (usize, Vec<String>)>::new();
        for generation in &snapshot.dense {
            if !embedding.space.compatible_with(&generation.space) {
                continue;
            }
            output.generation_ref = Some(generation.generation_id);
            let limit = plan.lane_budget(EvidenceFamily::Dense);
            let matches =
                domain_dense_matches(generation, &embedding.vector, limit, &query.projection)?;
            for (rank, item) in matches.into_iter().enumerate() {
                let Some(record) = item.record else {
                    continue;
                };
                let entry = best
                    .entry(record.reference)
                    .or_insert((rank + 1, Vec::new()));
                entry.0 = entry.0.min(rank + 1);
                entry
                    .1
                    .push(format!("dense:{}", generation.space.space_hash));
            }
        }
        if output.generation_ref.is_some() {
            output.status = LaneStatus::Ready;
            let mut values = best.into_iter().collect::<Vec<_>>();
            values.sort_by(|left, right| {
                left.1
                    .0
                    .cmp(&right.1.0)
                    .then_with(|| left.0.to_string().cmp(&right.0.to_string()))
            });
            output.candidates = values
                .into_iter()
                .take(plan.lane_budget(EvidenceFamily::Dense))
                .map(|(reference, (rank, variants))| LaneCandidate {
                    reference,
                    rank: rank as u32,
                    variants,
                    provider_metadata: serde_json::Value::Null,
                })
                .collect();
        } else {
            output
                .diagnostics
                .push("no dense generation matches the bound embedding space".into());
        }
    } else {
        output.diagnostics.push(embedding_error.unwrap_or_else(|| {
            if !provider_available {
                "text embedding provider is unavailable".into()
            } else {
                "dense serving generation is unavailable".into()
            }
        }));
    }
    Ok(output)
}

async fn prepare_signals(
    snapshot: &ServingSnapshot,
    query: &nous_core::CognitiveQuery,
    enabled_lanes: &[EvidenceFamily],
    plan: &QueryPlan,
    embedding_text: &str,
    provider: Option<&dyn TextEmbeddingProvider>,
    prepared_embedding: Option<&nous_runtime::QuerySemanticEmbedding>,
) -> Result<PreparedQuerySignals> {
    let query_text = text_query(query);
    let lexical = enabled_lanes
        .contains(&EvidenceFamily::Lexical)
        .then(|| prepare_lexical(snapshot, query, &query_text, plan))
        .transpose()?;

    let dense_enabled = enabled_lanes.contains(&EvidenceFamily::Dense);
    let cognitive_embedding =
        plan.expand_topology && plan.cognitive_profile.requirements().query_embedding;

    let mut embedding = None;
    let mut embedding_error = None;
    // Generation absence preserves the existing unavailable-lane semantics.
    // FORBIDDEN is checked before reaching the sole provider invocation.
    if (dense_enabled || cognitive_embedding)
        && !embedding_text.trim().is_empty()
        && query.capabilities.text_embedding != RequirementStrength::Forbidden
        && (!snapshot.dense.is_empty()
            || (cognitive_embedding
                && snapshot.vcp.as_ref().is_some_and(|_| {
                    matches!(
                        plan.cognitive_profile,
                        nous_runtime::CognitiveProfile::VcpDtsc
                            | nous_runtime::CognitiveProfile::VcpRiverMemo
                    )
                })))
    {
        if let Some(material) = prepared_embedding {
            embedding = Some(TextEmbeddingOutput {
                vector: material.vector.clone(),
                space: material.space.clone(),
                producer: material.producer.clone(),
            });
        } else if let Some(provider) = provider {
            match provider
                .embed(TextEmbeddingRequest {
                    subject: query.subject,
                    text: embedding_text.to_owned(),
                    query: true,
                })
                .await
            {
                Ok(value) => embedding = Some(value),
                Err(error) => embedding_error = Some(error.to_string()),
            }
        } else {
            embedding_error = Some("text embedding provider is unavailable".into());
        }
    }
    let dense = dense_enabled
        .then(|| {
            prepare_dense(
                snapshot,
                query,
                &query_text,
                plan,
                embedding.as_ref(),
                embedding_error,
                provider.is_some(),
            )
        })
        .transpose()?;

    Ok(PreparedQuerySignals {
        query_text,
        embedding,
        lexical,
        dense,
    })
}

#[async_trait::async_trait]
impl SharedLaneProvider for ServingService {
    async fn activate(&self, bound: &BoundQuery) -> Result<nous_runtime::QueryActivation> {
        crate::concept::lane::activate(
            &self.publisher.snapshot_for(bound.source_query.subject),
            bound,
            self.embedding().map(|p| p.as_ref()),
        )
        .await
    }
    async fn lanes(&self, bound: &BoundQuery, plan: &QueryPlan) -> Result<Vec<LaneOutput>> {
        let _reader = self.read_gate.clone().read_owned().await;
        let snapshot = self.publisher.snapshot_for(bound.source_query.subject);
        self.lanes_from_snapshot(
            &snapshot,
            bound,
            plan,
            self.embedding().map(|provider| provider.as_ref()),
        )
        .await
    }
}

impl ServingService {
    async fn lanes_from_snapshot(
        &self,
        snapshot: &ServingSnapshot,
        bound: &BoundQuery,
        plan: &QueryPlan,
        provider: Option<&dyn TextEmbeddingProvider>,
    ) -> Result<Vec<LaneOutput>> {
        if bound
            .historical_authority
            .as_ref()
            .is_some_and(|view| snapshot.view_digest.as_ref() != Some(&view.snapshot_digest))
        {
            return Err(Error::Unavailable(
                "candidate generation requires a compatible historical Serving view".into(),
            ));
        }
        let signals = prepare_signals(
            snapshot,
            &bound.source_query,
            &bound.enabled_lanes,
            plan,
            &bound.representation.text,
            provider,
            bound.activation.embedding.as_ref(),
        )
        .await?;
        let topology = if bound.lane_enabled(EvidenceFamily::TopologyWave) {
            Some(
                if plan.expand_topology
                    && matches!(
                        plan.cognitive_profile,
                        nous_runtime::CognitiveProfile::VcpDtsc
                            | nous_runtime::CognitiveProfile::VcpRiverMemo
                    )
                {
                    self.vcp_lane(snapshot, bound, plan, &signals).await?
                } else {
                    topology_lane(snapshot, bound, plan, &signals)?
                },
            )
        } else {
            None
        };
        let mut outputs = signals.into_lanes();
        outputs.extend(topology);
        if bound.lane_enabled(EvidenceFamily::TagDirect) {
            outputs.push(crate::concept::lane::direct_lane(snapshot, bound, plan));
        }
        Ok(outputs)
    }
}

fn domain_dense_matches(
    generation: &DenseGeneration,
    vector: &[f32],
    limit: usize,
    projection: &nous_core::ResultProjection,
) -> Result<Vec<DenseMatch>> {
    if projection.domain_names().is_empty() {
        return generation.search(vector, limit);
    }
    let allowed = generation
        .records()
        .filter(|record| projection.allows_reference(&record.reference))
        .filter_map(|record| u32::try_from(record.serving_doc_id).ok())
        .collect::<roaring::RoaringBitmap>();
    generation.search_filtered(vector, limit, &allowed)
}

#[cfg(test)]
#[path = "../tests/unit/query_signals.rs"]
mod tests;

/// Request-scoped immutable Serving view, retained through final validation.
pub struct ServingQuery {
    pub(crate) service: ServingService,
    pub(crate) snapshot: Arc<ServingSnapshot>,
    pub(crate) embedding: Option<crate::provider::RequestEmbedding>,
}
impl std::fmt::Debug for ServingQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServingQuery")
            .field("generation", &self.snapshot.generation)
            .finish()
    }
}
impl nous_runtime::QueryReadLease for ServingQuery {}
impl nous_runtime::QueryActivationView for ServingQuery {
    fn generation_trace(&self) -> QueryGenerationTrace {
        self.snapshot.generation_trace()
    }
    fn provider(&self) -> &dyn SharedLaneProvider {
        self
    }
}
#[async_trait::async_trait]
impl SharedLaneProvider for ServingQuery {
    async fn activate(&self, bound: &BoundQuery) -> Result<nous_runtime::QueryActivation> {
        crate::concept::lane::activate(
            &self.snapshot,
            bound,
            self.embedding
                .as_ref()
                .map(|p| p as &dyn TextEmbeddingProvider),
        )
        .await
    }
    async fn lanes(&self, bound: &BoundQuery, plan: &QueryPlan) -> Result<Vec<LaneOutput>> {
        self.service
            .lanes_from_snapshot(
                &self.snapshot,
                bound,
                plan,
                self.embedding
                    .as_ref()
                    .map(|provider| provider as &dyn TextEmbeddingProvider),
            )
            .await
    }
}
impl ServingService {
    pub async fn prepare_query(
        &self,
        bound: &BoundQuery,
        plan: &QueryPlan,
    ) -> Result<(ProjectionStatus, Arc<ServingQuery>)> {
        if let Some(view) = bound.historical_authority.as_deref() {
            return self.prepare_historical_query(bound, plan, view).await;
        }
        let lease = self.read_gate.clone().read_owned().await;
        let mut need = plan.serving_need(&bound.source_query);
        if bound.source_query.capabilities.text_embedding == RequirementStrength::Forbidden
            && plan.cognitive_profile.requirements().query_embedding
        {
            need.topology = false;
        }
        let status = self
            .prepare_with_snapshot(bound.source_query.subject, need, &bound.config_snapshot)
            .await?;
        // Resolve the exact generations prepared for this request. A concurrent
        // profile switch must not replace this query's immutable view.
        let records = self
            .store
            .serving_reusable(bound.source_query.subject)
            .await?;
        let current = self.publisher.snapshot_for(bound.source_query.subject);
        let mut snapshot = current.as_ref().clone();
        for record in records.iter().filter(|record| {
            status
                .generations
                .values()
                .any(|id| *id == record.generation_id)
        }) {
            if current.contains_generation(record.generation_id) {
                continue;
            }
            snapshot.install(self.open_record(record)?);
        }
        snapshot.retain_generations(status.generations.values().copied());
        let reader = self.query_reader(bound, snapshot)?;
        drop(lease);
        Ok((status, reader))
    }
    pub(crate) fn query_reader(
        &self,
        bound: &BoundQuery,
        snapshot: ServingSnapshot,
    ) -> Result<Arc<ServingQuery>> {
        let reader = Arc::new(ServingQuery {
            service: self.clone(),
            snapshot: Arc::new(snapshot),
            embedding: self.embedding().map(|provider| {
                crate::provider::RequestEmbedding::new(
                    provider.clone(),
                    bound.representation.text.clone(),
                )
            }),
        });
        let mut readers = self
            .query_readers
            .lock()
            .map_err(|_| Error::Infrastructure("serving reader registry unavailable".into()))?;
        readers.retain(|reader| reader.strong_count() > 0);
        readers.push(Arc::downgrade(&reader));
        drop(readers);
        Ok(reader)
    }
}
