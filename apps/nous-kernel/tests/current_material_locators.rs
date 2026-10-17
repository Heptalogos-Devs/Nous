// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use nous_core::*;
use nous_material::{DerivationInput, DerivedRepresentation, ObservationMaterial};
use nous_runtime::ContextResolver;
use test_support::longitudinal::{consolidation_producer, observation};
use test_support::query::subject;
use test_support::{database, open_runtime};

#[tokio::test]
async fn current_locator_requires_actual_occurrence_lineage_and_current_scope() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let mut observations = Vec::new();
    for text in ["message A", "artifact B"] {
        let mut input = observation(subject, None);
        input.occurrence.conversation_ref = Some("conversation:fixture:shared".into());
        input.material = ObservationMaterial::InlineText {
            text: text.into(),
            media_type: "text/plain".into(),
        };
        observations.push(runtime.material.record_observation(input).await.unwrap());
    }
    let occurrence_a = observations[0].occurrence.occurrence_id;
    let occurrence_b = observations[1].occurrence.occurrence_id;
    let representation = runtime
        .material
        .persist_derived_representation(
            DerivedRepresentation {
                formed_stamp: None,
                derived_representation_id: DerivedRepresentationId::new(),
                subject_id: subject,
                inputs: vec![DerivationInput {
                    ordinal: 0,
                    reference: CognitiveRef::SourceRegion(
                        observations[1]
                            .source_region
                            .as_ref()
                            .unwrap()
                            .source_region_id,
                    ),
                    role: "source".into(),
                }],
                strategy: "deterministic-test".into(),
                representation_kind: RepresentationKind::ImageDescription,
                producer: consolidation_producer(),
                revision: 1,
                payload_text: Some("Existing READY eight-frame interpretation".into()),
                payload_json: None,
                payload_artifact_id: None,
                quality: serde_json::json!({"frame_count": 8}),
                created_at: chrono::Utc::now(),
                supersedes: None,
            },
            None,
        )
        .await
        .unwrap();
    let scope = |occurrences: Vec<OccurrenceId>| SourceReadScope {
        subject_id: Some(subject),
        source_refs: occurrences
            .into_iter()
            .map(CognitiveRef::Occurrence)
            .collect(),
        origin_conversation_refs: vec![],
    };
    let mut both_scope = scope(vec![occurrence_a, occurrence_b]);
    both_scope
        .origin_conversation_refs
        .push("conversation:fixture:shared".into());
    let both = ContextResolver::source_read_view(&runtime, subject, &both_scope)
        .await
        .unwrap();
    let pair = EvidenceRef {
        occurrence_id: occurrence_b,
        locator: EvidenceLocator::DerivedRepresentation(representation.derived_representation_id),
        basis_role: BasisRole::Interpretation,
        epistemic_relation: None,
    };
    let selected = runtime
        .material
        .current_material_sources(subject, std::slice::from_ref(&pair), 4096, &both)
        .await
        .unwrap();
    let selected = selected[0].as_ref().unwrap();
    assert!(selected.text.as_ref().unwrap().contains("eight-frame"));
    assert!(
        selected
            .evidence
            .iter()
            .any(|e| e.basis_role == "actual_occurrence"
                && e.reference == CognitiveRef::Occurrence(occurrence_b))
    );
    let wrong = EvidenceRef {
        occurrence_id: occurrence_a,
        ..pair.clone()
    };
    let error = runtime
        .material
        .current_material_sources(subject, &[wrong], 4096, &both)
        .await
        .err()
        .expect("mismatched locator must be rejected");
    assert!(
        error
            .to_string()
            .contains("locator source does not match occurrence")
    );
    let only_a = ContextResolver::source_read_view(&runtime, subject, &scope(vec![occurrence_a]))
        .await
        .unwrap();
    let denied = runtime
        .material
        .current_material_sources(subject, &[pair], 4096, &only_a)
        .await
        .unwrap();
    assert!(denied[0].is_err());
}
