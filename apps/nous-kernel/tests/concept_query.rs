// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
mod concept_query {
    use super::*;
    pub(super) mod configuration;
    pub(super) mod historical;
    pub(super) mod model;
}
use concept_query::{
    configuration::check_activation_configuration, historical::check_historical_profiles,
    model::check_model_catalog_activity,
};
use nous_core::*;
use nous_kernel::{NousRuntime, RuntimeOptions};
use nous_memory::{CreateTagRequest, ReviseTagInput, TagContent, TagExpectation};
use nous_retrieval::{
    ServingOptions, TextEmbeddingOutput, TextEmbeddingProvider, TextEmbeddingRequest,
};
use nous_runtime::{COGNITIVE_PROFILE, CONCEPT_ENRICHMENT};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use std::sync::{Arc, Mutex};
use test_support::*;
#[derive(Default)]
struct Probe {
    calls: Mutex<Vec<(bool, String)>>,
}
#[async_trait::async_trait]
impl TextEmbeddingProvider for Probe {
    fn space(&self) -> EmbeddingSpaceSignature {
        EmbeddingSpaceSignature {
            space_hash: "concept-test".into(),
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

            signature_hash: "concept-probe".into(),
            provider_class: "deterministic_test".into(),
            operation: CapabilityOperation::TextEmbedding,
            implementation: "concept-test".into(),
            model_identity: None,
            model_revision: None,
            output_schema_digest: None,
            preprocessing_identity: "utf8".into(),
            preprocessing_revision: "1".into(),
            config_digest: "test".into(),
        }
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<TextEmbeddingOutput> {
        let vector = if request.text.contains("orthogonal policy probe") {
            vec![0.0, 1.0]
        } else {
            vec![1.0, 0.0]
        };
        self.calls
            .lock()
            .unwrap()
            .push((request.query, request.text));
        Ok(TextEmbeddingOutput {
            vector,
            space: self.space(),
            producer: self.producer(),
        })
    }
}
fn query(subject: SubjectId, mut cues: Vec<Cue>) -> CognitiveQuery {
    if !cues.iter().any(|cue| matches!(cue, Cue::Text(_))) {
        cues.insert(
            0,
            Cue::Text(TextCue {
                text: "Recall the selected concepts".into(),
            }),
        );
    }
    CognitiveQuery {
        subject,
        session: None,
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues,
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: DiagnosticsRequest::Full,
    }
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one integration scenario distinguishes bare/direct/explore, shared vectors and ephemeral enrichment"
)]
async fn direct_concept_recall_is_independent_and_vectors_are_shared_across_profiles() {
    let (root, url, _pg) = database().await;
    let probe = Arc::new(Probe::default());
    let rt=NousRuntime::open(RuntimeOptions {postgres_url:url,max_connections:4,acquire_timeout_ms:15000,object_root:root.path().join("objects").to_string_lossy().into_owned(),serving_options:ServingOptions {root:root.path().join("serving"),lexical:true,dense:true,topology:true,memory_enabled:true},embedding:Some(probe.clone()),stored_embedding:None,core_descriptors:vec![],deployment_document:serde_json::json!({"capabilities":{"process":{"memory":true},"subject_defaults":{"memory":true}},"serving":{"lexical":{"enabled":true},"dense":{"enabled":true},"topology":{"enabled":true}}})}).await.unwrap();
    let subject = rt
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            },
            metadata: serde_json::json!({}),
            capabilities: None,
        })
        .await
        .unwrap()
        .subject_id;
    let owner = rt.require_memory().unwrap();
    let tag = owner
        .create_tag(
            subject,
            CreateTagRequest {
                operation_id: OperationId::new(),
                label: "reader reclamation".into(),
                description: Some(
                    "Wait for all active readers before freeing the old generation".into(),
                ),
                kind_hint: Some("procedure".into()),
                origin: "host_explicit".into(),
                producer: None,
            },
        )
        .await
        .unwrap();
    let observed = observation(
        &rt,
        subject,
        "Reader lease retirement safely frees generation assets",
    )
    .await;
    let mut input = form_input(
        subject,
        observed.occurrence.occurrence_id,
        OperationId::new(),
        "Reader lease retirement safely frees generation assets",
    );
    input.tags = vec![tag.tag_id];
    let memory = owner.form_memory(input).await.unwrap();
    let revision = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    let text = Cue::Text(TextCue {
        text: "reader lease retirement".into(),
    });
    let bare = rt
        .execute_query(query(subject, vec![text.clone()]), None)
        .await
        .unwrap();
    assert!(
        bare.result
            .results
            .iter()
            .any(|hit| hit.reference == revision)
    );
    assert!(bare.bound.activation.inferred_tags.is_empty());
    assert_eq!(bare.bound.activation.model_calls, 0);
    assert!(
        !bare
            .bound
            .enabled_lanes
            .contains(&EvidenceFamily::TagDirect)
    );
    assert!(
        !bare
            .bound
            .enabled_lanes
            .contains(&EvidenceFamily::TopologyWave)
    );
    assert!(
        rt.serving
            .publisher
            .snapshot_for(subject)
            .concept
            .is_empty()
    );
    let mut direct = query(subject, vec![Cue::Tag(TagCue { tag: tag.tag_id })]);
    direct.capabilities.text_embedding = RequirementStrength::Forbidden;
    let direct = rt.execute_query(direct, None).await.unwrap();
    assert_eq!(direct.result.results.len(), 1);
    assert_eq!(direct.result.results[0].reference, revision);
    assert!(
        direct.result.results[0]
            .match_evidence
            .families
            .contains(&EvidenceFamily::TagDirect)
    );
    assert!(
        !direct
            .bound
            .enabled_lanes
            .contains(&EvidenceFamily::TopologyWave)
    );
    assert!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, text)| !text.starts_with("Concept:"))
    );
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            CONCEPT_ENRICHMENT.path(),
            serde_json::json!("existing"),
            None,
        )
        .await
        .unwrap();
    let before = rt.store.authority_seq(subject).await.unwrap();
    let count = probe
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(query, _)| *query)
        .count();
    let enriched = rt
        .execute_query(query(subject, vec![text.clone()]), None)
        .await
        .unwrap();
    assert!(
        enriched
            .bound
            .activation
            .inferred_tags
            .iter()
            .any(|activation| activation.tag == tag.tag_id)
    );
    assert_eq!(enriched.bound.activation.model_calls, 0);
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);
    assert_eq!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(query, _)| *query)
            .count(),
        count + 1
    );
    let calls_before_prepare = probe.calls.lock().unwrap().len();
    let prepared = rt
        .cognition
        .bind_query(query(subject, vec![text.clone()]))
        .await
        .unwrap();
    assert_eq!(probe.calls.lock().unwrap().len(), calls_before_prepare);
    assert!(prepared.activation.inferred_tags.is_empty());
    let mut forbidden = query(subject, vec![text.clone()]);
    forbidden.capabilities.text_embedding = RequirementStrength::Forbidden;
    let degraded = rt.execute_query(forbidden, None).await.unwrap();
    assert!(
        degraded
            .result
            .results
            .iter()
            .any(|hit| hit.reference == revision)
    );
    assert!(degraded.bound.activation.inferred_tags.is_empty());
    assert!(
        degraded
            .bound
            .activation
            .degradation
            .iter()
            .any(|d| d.code == "concept_enrichment_unavailable")
    );
    assert_eq!(probe.calls.lock().unwrap().len(), calls_before_prepare);
    let concept_id = enriched.bound.activation.concept_generation.unwrap();
    for profile in [
        "vcp-dtsc-v9.2.1-adapter-v1",
        "vcp-rivermemo-v3.1-adapter-v1",
        "nous-node-potential-v1",
    ] {
        rt.configuration
            .set_subject_override(
                OperationId::new(),
                subject,
                COGNITIVE_PROFILE.path(),
                serde_json::json!(profile),
                None,
            )
            .await
            .unwrap();
        let mut explored = query(
            subject,
            vec![text.clone(), Cue::Tag(TagCue { tag: tag.tag_id })],
        );
        explored.exploration = ExplorationIntent::BoundedAssociative;
        let result = rt.execute_query(explored, None).await.unwrap();
        assert!(
            result
                .bound
                .enabled_lanes
                .contains(&EvidenceFamily::TopologyWave)
        );
        let snapshot = rt.serving.publisher.snapshot_for(subject);
        let concept = snapshot
            .concept
            .iter()
            .find(|generation| generation.space.is_some())
            .unwrap();
        assert_eq!(concept.generation_id, concept_id);
        if let Some(vcp) = &snapshot.vcp {
            let id = vcp.identities.id(&CognitiveRef::Tag(tag.tag_id)).unwrap();
            assert_eq!(
                vcp.vectors.iter().find(|(key, _)| *key == id).unwrap().1,
                concept.record(tag.tag_id).unwrap().vector.clone().unwrap()
            );
        }
    }
    assert_eq!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(query, text)| !*query && text.starts_with("Concept:"))
            .count(),
        1
    );
    let old_snapshot = rt.serving.publisher.snapshot_for(subject);
    let historical_cut = rt.cognition.now(subject);
    let revised = owner
        .revise_tag(
            subject,
            ReviseTagInput {
                operation_id: OperationId::new(),
                target: TagExpectation {
                    tag_id: tag.tag_id,
                    expected_revision_id: tag.current_revision_id,
                },
                content: TagContent {
                    label: "reader reclamation".into(),
                    description: Some(
                        "Retain immutable historical assets until every reader releases its lease"
                            .into(),
                    ),
                    kind_hint: Some("procedure".into()),
                },
                producer: None,
            },
        )
        .await
        .unwrap();
    let updated = rt
        .execute_query(query(subject, vec![text]), None)
        .await
        .unwrap();
    assert_ne!(
        updated.bound.activation.concept_generation,
        Some(concept_id)
    );
    let snapshot = rt.serving.publisher.snapshot_for(subject);
    let concept = snapshot
        .concept
        .iter()
        .find(|g| g.generation_id == updated.bound.activation.concept_generation.unwrap())
        .unwrap();
    assert_eq!(
        concept.record(tag.tag_id).unwrap().revision,
        revised.current_revision_id
    );
    assert!(
        concept
            .record(tag.tag_id)
            .unwrap()
            .semantic
            .text
            .contains("historical assets")
    );
    assert!(
        old_snapshot
            .concept
            .iter()
            .find(|g| g.generation_id == concept_id)
            .unwrap()
            .record(tag.tag_id)
            .unwrap()
            .semantic
            .text
            .contains("active readers")
    );
    assert_eq!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(query, text)| !*query && text.starts_with("Concept:"))
            .count(),
        2
    );
    let semantic = rt
        .execute_query(
            query(
                subject,
                vec![Cue::Concept(ConceptCue {
                    text: "active reader reclamation".into(),
                })],
            ),
            None,
        )
        .await
        .unwrap();
    assert!(
        semantic
            .bound
            .enabled_lanes
            .contains(&EvidenceFamily::Lexical)
    );
    assert!(
        semantic
            .bound
            .enabled_lanes
            .contains(&EvidenceFamily::Dense)
    );
    assert!(
        semantic
            .bound
            .representation
            .text
            .contains("Semantic concepts:")
    );
    assert!(
        semantic
            .bound
            .activation
            .inferred_tags
            .iter()
            .any(|activation| activation.tag == tag.tag_id)
    );
    assert!(
        semantic
            .result
            .results
            .iter()
            .any(|hit| hit.reference == revision)
    );
    check_model_catalog_activity(&rt, subject, tag.tag_id, &probe).await;
    check_historical_profiles(&rt, subject, tag.tag_id, &revision, historical_cut).await;
    Box::pin(check_activation_configuration(&rt, subject)).await;
}
