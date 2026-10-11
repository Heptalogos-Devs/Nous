// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct EmbeddingProbe {
    calls: AtomicUsize,
    fail: bool,
}

#[async_trait::async_trait]
impl TextEmbeddingProvider for EmbeddingProbe {
    fn space(&self) -> EmbeddingSpaceSignature {
        EmbeddingSpaceSignature {
            space_hash: "signals-space".into(),
            model_identity: "probe".into(),
            weights_revision: "1".into(),
            task: "retrieval".into(),
            input_representation: "text".into(),
            preprocessing_identity: "utf8".into(),
            preprocessing_revision: "1".into(),
            dimension: 2,
            normalization: "none".into(),
            output_semantics: "dense_vector".into(),
        }
    }
    fn producer(&self) -> ProducerSignature {
        ProducerSignature {
            model_role: None,
            model_profile: None,
            execution_profile: None,
            inference_controls_digest: None,
            role_policy_digest: None,
            prompt_id: None,
            prompt_digest: None,

            signature_hash: "signals-producer".into(),
            provider_class: "probe".into(),
            operation: CapabilityOperation::TextEmbedding,
            implementation: "probe".into(),
            model_identity: Some("probe".into()),
            model_revision: None,
            output_schema_digest: None,
            preprocessing_identity: "utf8".into(),
            preprocessing_revision: "1".into(),
            config_digest: "probe".into(),
        }
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<TextEmbeddingOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(request.query);
        assert_eq!(request.text, "query");
        if self.fail {
            return Err(Error::Unavailable("probe failure".into()));
        }
        Ok(TextEmbeddingOutput {
            vector: vec![1.0, 0.0],
            space: self.space(),
            producer: self.producer(),
        })
    }
}

fn query() -> CognitiveQuery {
    CognitiveQuery {
        projection: ResultProjection {
            domains: vec![ResultDomain::Memory],
        },
        temporal_frame: Default::default(),

        work_context: None,
        subject: SubjectId::new(),
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "query".into(),
            })],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}

fn snapshot(provider: &EmbeddingProbe) -> (ServingSnapshot, CognitiveRef) {
    let reference = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
    let generations = (0..2)
        .map(|index| {
            let mut generation = DenseGeneration::new(provider.space(), 1).expect("dense index");
            generation
                .insert(
                    VectorRecord {
                        serving_doc_id: index,
                        reference: reference.clone(),
                        embedding_space: provider.space(),
                        producer_signature: provider.producer().signature_hash,
                        representation_kind: "memory".into(),
                        source_region: None,
                    },
                    &[1.0, 0.0],
                )
                .expect("insert vector");
            Arc::new(generation)
        })
        .collect();
    (
        ServingSnapshot {
            dense: generations,
            ..Default::default()
        },
        reference,
    )
}

#[tokio::test]
async fn one_preparation_shares_embedding_and_deduplicates_dense_generations() {
    let provider = EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let (snapshot, reference) = snapshot(&provider);
    let query = query();
    let plan = QueryPlan::for_query(&query);
    let signals = prepare_signals(
        &snapshot,
        &query,
        &[EvidenceFamily::Dense],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("prepare");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(std::ptr::eq(
        signals.embedding().expect("embedding"),
        signals.embedding().expect("same embedding")
    ));
    assert_eq!(signals.query_text(), "query");
    let output = signals.dense().expect("dense");
    assert_eq!(output.status, LaneStatus::Ready);
    assert_eq!(output.candidates.len(), 1);
    assert_eq!(output.candidates[0].reference, reference);
    assert_eq!(output.candidates[0].variants.len(), 2);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn forbidden_and_missing_generation_do_not_call_embedding() {
    let provider = EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let (snapshot, _) = snapshot(&provider);
    let mut query = query();
    query.capabilities.text_embedding = RequirementStrength::Forbidden;
    let plan = QueryPlan::for_query(&query);
    let signals = prepare_signals(
        &snapshot,
        &query,
        &[EvidenceFamily::Dense],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("forbidden");
    assert!(signals.embedding().is_none());
    assert_eq!(signals.dense().expect("dense").status, LaneStatus::Ready);
    query.capabilities.text_embedding = RequirementStrength::Optional;
    let signals = prepare_signals(
        &ServingSnapshot::default(),
        &query,
        &[EvidenceFamily::Dense],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("missing");
    assert_eq!(
        signals.dense().expect("dense").status,
        LaneStatus::Unavailable
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn provider_failure_remains_unavailable_lane_for_runtime_requirement_policy() {
    let provider = EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: true,
    };
    let (snapshot, _) = snapshot(&provider);
    let query = query();
    let plan = QueryPlan::for_query(&query);
    let signals = prepare_signals(
        &snapshot,
        &query,
        &[EvidenceFamily::Dense],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("lane failure");
    assert!(signals.embedding().is_none());
    let dense = signals.dense().expect("dense");
    assert_eq!(dense.status, LaneStatus::Unavailable);
    assert!(dense.candidates.is_empty());
    assert!(dense.diagnostics[0].contains("probe failure"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cognitive_embedding_is_shared_even_when_dense_lane_is_disabled() {
    let provider = EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: false,
    };
    let (snapshot, tag) = vcp_snapshot(&provider);
    assert!(snapshot.dense.is_empty());
    let mut query = query();
    query.expression.cues.push(Cue::Tag(TagCue { tag }));
    let mut plan = QueryPlan::for_query(&query);
    plan.expand_topology = true;
    plan.cognitive_profile = nous_runtime::CognitiveProfile::VcpRiverMemo;
    let signals = prepare_signals(
        &snapshot,
        &query,
        &[EvidenceFamily::TopologyWave],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("cognitive signals");
    assert!(signals.embedding().is_some());
    assert!(signals.dense().is_none());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    query.capabilities.text_embedding = RequirementStrength::Forbidden;
    let signals = prepare_signals(
        &snapshot,
        &query,
        &[EvidenceFamily::TopologyWave],
        &plan,
        "query",
        Some(&provider),
        None,
    )
    .await
    .expect("forbidden cognitive signals");
    assert!(signals.embedding().is_none());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

fn vcp_snapshot(provider: &EmbeddingProbe) -> (ServingSnapshot, TagId) {
    let body = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
    let first = TagId::new();
    let second = TagId::new();
    let tag_a = CognitiveRef::Tag(first);
    let tag_b = CognitiveRef::Tag(second);
    let material = crate::VcpProjectionMaterial {
        authority_watermark: 1,
        space: provider.space(),
        producer: provider.producer(),
        identities: crate::VcpIdentityMap::new([body.clone(), tag_a.clone(), tag_b.clone()]),
        edges: Vec::new(),
        documents: vec![
            crate::VcpProjectedDocument {
                reference: body,
                representation_text: "body".into(),
                vector: vec![1.0, 0.0],
                concept_refs: vec![tag_a.clone(), tag_b.clone()],
                curve_order: crate::VcpCurveOrder::StableIdentity,
                evidence_roots: Default::default(),
            },
            crate::VcpProjectedDocument {
                reference: tag_a,
                representation_text: "query".into(),
                vector: vec![1.0, 0.0],
                concept_refs: Vec::new(),
                curve_order: crate::VcpCurveOrder::StableIdentity,
                evidence_roots: Default::default(),
            },
            crate::VcpProjectedDocument {
                reference: tag_b,
                representation_text: "other".into(),
                vector: vec![0.0, 1.0],
                concept_refs: Vec::new(),
                curve_order: crate::VcpCurveOrder::StableIdentity,
                evidence_roots: Default::default(),
            },
        ],
    };
    let assets = crate::VcpGeneration::build(
        ServingGenerationId::new(),
        nous_runtime::CognitiveProfile::VcpRiverMemo,
        material,
        crate::VcpAssetPolicy::default(),
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let generation = crate::VcpServingGeneration::create(assets, directory.path()).unwrap();
    (
        ServingSnapshot {
            vcp: Some(Arc::new(generation)),
            ..Default::default()
        },
        first,
    )
}

#[tokio::test]
async fn concurrent_leaf_requests_share_one_embedding_provider_invocation() {
    let provider = Arc::new(EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: false,
    });
    let shared = crate::provider::RequestEmbedding::new(provider.clone(), "query".into());
    let request = TextEmbeddingRequest {
        subject: SubjectId::new(),
        text: "query".into(),
        query: true,
    };
    let (a, b) = tokio::join!(shared.embed(request.clone()), shared.embed(request));
    assert_eq!(a.unwrap().vector, b.unwrap().vector);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn host_embedding_recovers_reader_after_preparation_without_material() {
    let provider = Arc::new(EmbeddingProbe {
        calls: AtomicUsize::new(0),
        fail: true,
    });
    let shared = crate::provider::RequestEmbedding::new(provider.clone(), "query".into());
    let request = TextEmbeddingRequest {
        subject: SubjectId::new(),
        text: "query".into(),
        query: true,
    };
    assert!(shared.embed(request.clone()).await.is_err());
    let supplied = TextEmbeddingOutput {
        vector: vec![1.0, 0.0],
        space: provider.space(),
        producer: provider.producer(),
    };
    let result = crate::material::with_query_material(
        vec![crate::material::QueryEmbedding {
            text: "query".into(),
            output: supplied.clone(),
        }],
        shared.embed(request),
    )
    .await
    .expect("execution receives Host material after preparation");
    assert_eq!(result.vector, supplied.vector);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}
