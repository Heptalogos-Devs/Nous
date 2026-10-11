// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use chrono::Utc;
use nous_core::{
    CapabilityOperation, CognitiveRef, DerivedRepresentationId, ProducerSignature,
    RepresentationKind, SubjectId,
};
use nous_material::{DerivationInput, DerivedRepresentation};
use nous_subject::{CognitiveSeedInput, CreateSubject};

async fn assert_representation_exposes_gettable_producer(
    runtime: &nous_kernel::NousRuntime,
    subject: SubjectId,
    representation_id: DerivedRepresentationId,
) {
    use nous_protocol::nous::wave::v1alpha1 as p;
    use p::material_service_server::MaterialService;

    let producer_id: uuid::Uuid = sqlx::query_scalar(
        "SELECT producer_signature_id FROM derived_representations WHERE derived_representation_id=$1",
    )
    .bind(representation_id.0)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    let producer_id = producer_id.to_string();
    let service = nous_kernel::transport::KernelService(runtime.clone());
    let representations = service
        .list_derived_representations(tonic::Request::new(p::RepresentationListRequest {
            subject_id: subject.0.to_string(),
            source_region_id: None,
            artifact_id: None,
            kind: None,
            limit: 20,
        }))
        .await
        .unwrap()
        .into_inner();
    let representation = representations
        .items
        .into_iter()
        .find(|value| value.representation_id == representation_id.0.to_string())
        .unwrap();
    assert_eq!(
        representation.producer_signature_id.as_deref(),
        Some(producer_id.as_str())
    );
    let producer = service
        .get_producer(tonic::Request::new(p::ObjectRequest {
            subject_id: subject.0.to_string(),
            id: producer_id,
        }))
        .await
        .unwrap()
        .into_inner();
    let producer_hash = representation.producer.unwrap().signature_hash;
    assert_eq!(representation.producer_signature, producer_hash);
    assert_eq!(producer.signature_hash, producer_hash);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one database journey checks shared provenance, stable segments, structured payload and schema-bound replay"
)]
async fn ordered_derivation_graph_preserves_roots_and_reuses_success() {
    let (root, url, _postgres) = test_support::database().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: nous_core::SubjectId::default(),
            operation_id: nous_core::OperationId::new(),
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
    let observed = test_support::observation(&runtime, subject, "source facts").await;
    let repeated = test_support::observation(&runtime, subject, "source facts").await;
    let artifact = observed.artifact.as_ref().unwrap().artifact_id;
    assert_eq!(repeated.artifact.as_ref().unwrap().artifact_id, artifact);
    assert_ne!(
        observed.occurrence.occurrence_id,
        repeated.occurrence.occurrence_id
    );
    let (origins, truncated) = runtime
        .material
        .occurrence_ids_for_artifact(subject, artifact, 1)
        .await
        .unwrap();
    assert_eq!(origins.len(), 1);
    assert!(truncated);
    let (origins, truncated) = runtime
        .material
        .occurrence_ids_for_artifact(subject, artifact, 20)
        .await
        .unwrap();
    assert_eq!(origins.len(), 2);
    assert!(origins.contains(&observed.occurrence.occurrence_id));
    assert!(origins.contains(&repeated.occurrence.occurrence_id));
    assert!(!truncated);
    assert!(
        runtime
            .material
            .occurrence_ids_for_artifact(nous_core::SubjectId::new(), artifact, 20)
            .await
            .is_err()
    );
    assert!(
        runtime
            .material
            .occurrence_ids_for_artifact(subject, artifact, 0)
            .await
            .is_err()
    );
    let source = observed.source_region.unwrap().source_region_id;
    let make = |input: CognitiveRef, kind| {
        DerivedRepresentation {
        derived_representation_id: DerivedRepresentationId::new(),
        subject_id: subject,
        inputs: vec![DerivationInput {
            ordinal: 0,
            reference: input.clone(),
            role: "source".into(),
        }],
        strategy: "describe_then_structure".into(),
        representation_kind: kind,
        producer: ProducerSignature {
            model_role: None,
            model_profile: None,
            execution_profile: None,
            inference_controls_digest: None,
            role_policy_digest: None,
            prompt_id: None,
            prompt_digest: None,

            signature_hash: "untrusted supplied hash".into(),
            provider_class: "deterministic-local-test".into(),
            operation: CapabilityOperation::TextInterpretation,
            implementation: "material-graph-test".into(),
            model_identity: None,
            model_revision: None,
            output_schema_digest: (kind == RepresentationKind::StructuredInterpretation).then(|| "a".repeat(64)),
            preprocessing_identity: "test".into(),
            preprocessing_revision: "1".into(),
            config_digest: "test-config".into(),
        },
        revision: 1,
        payload_text: Some("faithful representation".into()),
        payload_json: (kind == RepresentationKind::StructuredInterpretation).then(|| {
            let (kind,value)=nous_core::reference_parts(&input);
            serde_json::json!({"summary":{"content":"faithful representation","basis_refs":[{"kind":kind,"value":value}]},"coverage":{"visual":"not_available","audio":"not_available","embedded_text":"not_available","source_text":"observed"},"observations":[{"kind":"text","content":"source facts","evidence_channel":"source_text","basis":"direct","certainty":"clear","start_ms":null,"end_ms":null,"basis_refs":[{"kind":kind,"value":value}]}],"mentions":[],"embedded_text":[],"source_text":[],"speech":[],"interpretations":[],"uncertainties":[]})
        }),
        payload_artifact_id: None,
        quality: serde_json::json!({}),
        created_at: Utc::now(),
        supersedes: None,
    }
    };
    let description = runtime
        .material
        .persist_derived_representation(make(
            CognitiveRef::SourceRegion(source),
            RepresentationKind::Summary,
        ))
        .await
        .unwrap();
    assert_representation_exposes_gettable_producer(
        &runtime,
        subject,
        description.derived_representation_id,
    )
    .await;
    assert_ne!(
        description.producer.signature_hash,
        "untrusted supplied hash"
    );
    let segments = runtime
        .material
        .segment_description(subject, description.derived_representation_id)
        .await
        .unwrap();
    let replay_segments = runtime
        .material
        .segment_description(subject, description.derived_representation_id)
        .await
        .unwrap();
    assert_eq!(
        segments[0].region.derived_region_id,
        replay_segments[0].region.derived_region_id
    );
    assert_eq!(
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>(),
        description.payload_text.as_deref().unwrap()
    );
    let mut structured_input = make(
        CognitiveRef::DerivedRepresentation(description.derived_representation_id),
        RepresentationKind::StructuredInterpretation,
    );
    structured_input.payload_json.as_mut().unwrap()["observations"][0]["basis_refs"] = serde_json::json!([{"kind":"derived_region","value":segments[0].region.derived_region_id.0.to_string()}]);
    let expected_payload = structured_input.payload_json.clone();
    let structured = runtime
        .material
        .persist_derived_representation(structured_input)
        .await
        .unwrap();
    let saved_payload: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload_json FROM derived_representations WHERE derived_representation_id=$1",
    )
    .bind(structured.derived_representation_id.0)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(saved_payload, expected_payload);
    let mut changed_schema = make(
        CognitiveRef::DerivedRepresentation(description.derived_representation_id),
        RepresentationKind::StructuredInterpretation,
    );
    changed_schema.producer.output_schema_digest = Some("b".repeat(64));
    let changed_schema = runtime
        .material
        .persist_derived_representation(changed_schema)
        .await
        .unwrap();
    assert_ne!(
        changed_schema.derived_representation_id,
        structured.derived_representation_id
    );
    let roots: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT source_region_id FROM representation_source_regions($1,$2)")
            .bind(subject.0)
            .bind(structured.derived_representation_id.0)
            .fetch_all(runtime.store.pool())
            .await
            .unwrap();
    assert_eq!(roots, vec![source.0]);
    let evidence = |representation| {
        nous_core::RevisionBasis::Evidence(nous_core::EvidenceRef {
            epistemic_relation: None,
            occurrence_id: observed.occurrence.occurrence_id,
            locator: nous_core::EvidenceLocator::DerivedRepresentation(representation),
            basis_role: nous_core::BasisRole::Interpretation,
        })
    };
    assert_eq!(
        runtime
            .memory
            .as_ref()
            .unwrap()
            .provenance_summary(subject, &[evidence(structured.derived_representation_id)])
            .await
            .unwrap()
            .roots
            .len(),
        1
    );
    let other = test_support::observation(&runtime, subject, "independent source facts").await;
    let mut forged_support = make(
        CognitiveRef::DerivedRepresentation(description.derived_representation_id),
        RepresentationKind::StructuredInterpretation,
    );
    forged_support.payload_json.as_mut().unwrap()["observations"][0]["basis_refs"] = serde_json::json!([{"kind":"source_region","value":other.source_region.as_ref().unwrap().source_region_id.0.to_string()}]);
    assert!(
        runtime
            .material
            .persist_derived_representation(forged_support)
            .await
            .is_err()
    );
    let mut forged_summary = make(
        CognitiveRef::DerivedRepresentation(description.derived_representation_id),
        RepresentationKind::StructuredInterpretation,
    );
    forged_summary.payload_json.as_mut().unwrap()["summary"]["basis_refs"] = serde_json::json!([{"kind":"source_region","value":other.source_region.as_ref().unwrap().source_region_id.0.to_string()}]);
    assert!(
        runtime
            .material
            .persist_derived_representation(forged_summary)
            .await
            .is_err()
    );
    let mut combined = make(
        CognitiveRef::DerivedRepresentation(structured.derived_representation_id),
        RepresentationKind::Summary,
    );
    combined.inputs.push(DerivationInput {
        ordinal: 1,
        reference: CognitiveRef::SourceRegion(other.source_region.unwrap().source_region_id),
        role: "second source".into(),
    });
    let combined = runtime
        .material
        .persist_derived_representation(combined)
        .await
        .unwrap();
    assert_eq!(
        runtime
            .memory
            .as_ref()
            .unwrap()
            .provenance_summary(subject, &[evidence(combined.derived_representation_id)])
            .await
            .unwrap()
            .roots
            .len(),
        2
    );
    let mut retry = make(
        CognitiveRef::SourceRegion(source),
        RepresentationKind::Summary,
    );
    retry.payload_text = Some("different stochastic retry output".into());
    let retried = runtime
        .material
        .persist_derived_representation(retry)
        .await
        .unwrap();
    assert_eq!(
        retried.derived_representation_id,
        description.derived_representation_id
    );
    assert_eq!(retried.payload_text, description.payload_text);
    let mut duplicate = make(
        CognitiveRef::SourceRegion(source),
        RepresentationKind::Summary,
    );
    duplicate.inputs.push(DerivationInput {
        ordinal: 1,
        reference: CognitiveRef::SourceRegion(source),
        role: "duplicate".into(),
    });
    assert!(
        runtime
            .material
            .persist_derived_representation(duplicate)
            .await
            .is_err()
    );
    let mut cycle = make(
        CognitiveRef::SourceRegion(source),
        RepresentationKind::Summary,
    );
    cycle.inputs[0].reference =
        CognitiveRef::DerivedRepresentation(cycle.derived_representation_id);
    assert!(
        runtime
            .material
            .persist_derived_representation(cycle)
            .await
            .is_err()
    );
    let mut invalid = make(
        CognitiveRef::SourceRegion(source),
        RepresentationKind::Summary,
    );
    invalid.inputs[0].reference =
        CognitiveRef::DerivedRepresentation(DerivedRepresentationId::new());
    assert!(
        runtime
            .material
            .persist_derived_representation(invalid)
            .await
            .is_err()
    );
}
