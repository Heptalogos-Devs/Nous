// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::LongitudinalEmbedding;
use chrono::Duration;
use nous_core::{SourceClass, TemporalExtent};
use nous_kernel::{NousRuntime, RuntimeOptions};
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, RuntimeDirective,
};
use nous_memory::EpisodeView;
use nous_retrieval::ServingOptions;
use nous_runtime::{CognitiveClock, ManualCognitiveClock};
use std::sync::Arc;

pub(crate) async fn runtime_with_clock(
    url: &str,
    root: &tempfile::TempDir,
    clock: Arc<dyn CognitiveClock>,
) -> NousRuntime {
    runtime_with_clock_serving(url, root, clock, false).await
}

pub(crate) async fn runtime_with_clock_serving(
    url: &str,
    root: &tempfile::TempDir,
    clock: Arc<dyn CognitiveClock>,
    serving: bool,
) -> NousRuntime {
    NousRuntime::open_with_clock(RuntimeOptions {
        postgres_url: url.into(), max_connections: 8,
        acquire_timeout_ms: 15000,
        object_root: root.path().join("objects").to_string_lossy().into_owned(),
        serving_options: ServingOptions { root: root.path().join("serving"), lexical: serving, dense: serving, topology: false, memory_enabled: true },
        embedding: serving.then(|| Arc::new(LongitudinalEmbedding) as Arc<dyn nous_retrieval::TextEmbeddingProvider>), stored_embedding: None,
        core_descriptors: vec![],
        deployment_document: serde_json::json!({"serving":{"lexical":{"enabled":serving},"dense":{"enabled":serving},"topology":{"enabled":false}}}),
    }, clock).await.expect("open clock-injected runtime")
}

pub(crate) fn observation(
    subject: nous_core::SubjectId,
    session: Option<nous_core::SessionId>,
) -> ObservationInput {
    ObservationInput {
        subject,
        session,
        occurrence: OccurrenceDescriptor {
            source_class: SourceClass::Message,
            external_object_ref: None,
            occurred_time: TemporalExtent::Unknown,
            observed_at: None,
            conversation_ref: None,
            actor_entity_ref: None,
            context: serde_json::json!({}),
        },
        material: ObservationMaterial::InlineText {
            text: "durable experience".into(),
            media_type: "text/plain".into(),
        },
        entities: vec![],
        runtime: RuntimeDirective::default(),
    }
}

pub(crate) async fn journal_source_episode(
    rt: &NousRuntime,
    clock: &ManualCognitiveClock,
    subject: nous_core::SubjectId,
) -> EpisodeView {
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    for source in ["object:source-a", "object:source-b"] {
        let mut input = observation(subject, Some(session.session_id));
        input.material = ObservationMaterial::InlineText {
            text: if source == "object:source-a" {
                format!("{source}{}界", "x".repeat(2047 - source.len()))
            } else {
                source.into()
            },
            media_type: "text/plain".into(),
        };
        input.occurrence.external_object_ref = Some(nous_core::ObjectRef::new(source).unwrap());
        rt.material
            .record_observation_once(input, Some(uuid::Uuid::new_v4()))
            .await
            .unwrap();
        clock.advance_by(subject, Duration::seconds(1)).unwrap();
    }
    rt.organize_experience(subject, 128, true)
        .await
        .unwrap()
        .episodes
        .remove(0)
}

pub(crate) fn consolidation_producer() -> nous_core::ProducerSignature {
    nous_core::ProducerSignature {
        model_role: None,
        model_profile: None,
        execution_profile: None,
        inference_controls_digest: None,
        role_policy_digest: None,
        prompt_id: None,
        prompt_digest: None,

        signature_hash: String::new(),
        provider_class: "semantic-stub".into(),
        operation: nous_core::CapabilityOperation::MemoryConsolidationText,
        implementation: "longitudinal-owner-test".into(),
        model_identity: Some("stub".into()),
        model_revision: None,
        output_schema_digest: Some("a".repeat(64)),
        preprocessing_identity: "memory/consolidation.md".into(),
        preprocessing_revision: "b".repeat(64),
        config_digest: "c".repeat(64),
    }
}

pub(crate) fn consolidation_memory(episode: &EpisodeView) -> nous_memory::ExplicitMemoryInput {
    let nous_core::CognitiveRef::Occurrence(occurrence) = episode.members[0].reference else {
        panic!("expected occurrence")
    };
    nous_memory::ExplicitMemoryInput {
        producer: Some(consolidation_producer()),
        operation_id: nous_core::OperationId::new(),
        subject: episode.object.subject_id,
        tags: Vec::new(),
        cognitive_role: nous_memory::CognitiveRole::Declarative,
        formation_mode: nous_memory::FormationMode::Grounded,
        grounding_occurrence_id: Some(occurrence),
        semantic_role: "statement".into(),
        representation_text: "A reusable observed fact.".into(),
        title: None,
        basis: vec![nous_core::RevisionBasis::Evidence(nous_core::EvidenceRef {
            epistemic_relation: None,
            occurrence_id: occurrence,
            locator: nous_core::EvidenceLocator::WholeOccurrence,
            basis_role: nous_core::BasisRole::Direct,
        })],
        aboutness: vec![],
        valid_time: TemporalExtent::Unknown,
        epistemic_class: nous_core::EpistemicClass::Derived,
    }
}

pub(crate) fn consolidation_schema(episode: &EpisodeView) -> nous_memory::CreateSchemaInput {
    nous_memory::CreateSchemaInput {
        operation_id: nous_core::OperationId::new(),
        subject: episode.object.subject_id,
        content: nous_memory::SchemaContent {
            producer: Some(consolidation_producer()),
            title: Some("Recurring pattern".into()),
            structural_claim: "Two independent sources describe a recurring pattern.".into(),
            applicability_scope: nous_memory::SchemaScope {
                description: "The observed contexts".into(),
                aboutness: vec![],
                tags: vec![],
                valid_time: TemporalExtent::Unknown,
            },
            boundary_definition: "Applies to these observed contexts.".into(),
            formation_kind: nous_memory::SchemaFormationKind::Synthesized,
            evidence_links: episode
                .basis
                .iter()
                .cloned()
                .map(|basis| nous_memory::SchemaEvidenceLinkInput {
                    role: nous_memory::SchemaEvidenceRole::Support,
                    basis,
                })
                .collect(),
        },
    }
}
