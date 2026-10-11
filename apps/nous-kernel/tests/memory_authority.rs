// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::Utc;
use nous_core::{
    CognitiveRef, EntityRef, EpistemicClass, OperationId, QueryTarget, TemporalExtent, UseEventId,
};
use nous_memory::{
    AssociationBasis, AssociationBasisClass, AssociationPolarity, BasisRole, CreateSchemaInput,
    EvidenceLocator, EvidenceRef, FormationMode, RevisionBasis, SchemaEvidenceLinkInput,
    SchemaEvidenceRole, SchemaFormationKind, SchemaScope, UseEventRef,
};
use nous_runtime::{UseFeedback, UseFeedbackEvent, UseKind};
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime, open_runtime_with_serving};

#[tokio::test]
async fn self_dependent_revision_is_invalid_and_preserves_current_memory() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "A supported source fact").await;
    let owner = runtime.require_memory().unwrap();
    let original = owner
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "A supported source fact",
        ))
        .await
        .unwrap();
    let sequence = runtime.store.authority_seq(subject).await.unwrap();
    let result = owner
        .revise_memory(nous_memory::ReviseMemoryInput {
            producer: None,
            operation_id: OperationId::new(),
            subject,
            memory_id: original.object.memory_id,
            expected_object_epoch: original.object.object_epoch,
            intent: nous_memory::RevisionIntent::Correct,
            formation_mode: FormationMode::Grounded,
            grounding_occurrence_id: Some(source.occurrence.occurrence_id),
            semantic_role: "fact".into(),
            representation_text: "A revised source fact".into(),
            title: None,
            basis: vec![RevisionBasis::CognitionDependency(
                nous_core::CognitionDependency {
                    epistemic_relation: None,
                    target_revision: CognitiveRef::MemoryRevision(
                        original.revision.memory_revision_id,
                    ),
                    basis_role: BasisRole::Direct,
                },
            )],
            aboutness: vec![],
            valid_time: TemporalExtent::Unknown,
            epistemic_class: EpistemicClass::Observed,
        })
        .await;
    assert!(
        matches!(result, Err(nous_core::Error::Invalid(message)) if message.contains("object cycle"))
    );
    let current = owner
        .memory(subject, original.object.memory_id, None)
        .await
        .unwrap();
    assert_eq!(
        current.revision.memory_revision_id,
        original.revision.memory_revision_id
    );
    assert_eq!(current.object.object_epoch, original.object.object_epoch);
    assert_eq!(
        runtime.store.authority_seq(subject).await.unwrap(),
        sequence
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one temporary Schema cohort checks visibility and replay across lifecycle changes and irreversible purge without redundant database setup"
)]
async fn schema_lifecycle_fences_queries_and_purge_cannot_replay_content() {
    use nous_memory::schema::SchemaLifecycleAction as Action;
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let source = observation(
        &runtime,
        subject,
        "Temporary operating rule: open tasks can be continued; ended tasks cannot.",
    )
    .await;
    let owner = runtime.require_memory().unwrap();
    let schema = owner
        .create_schema(CreateSchemaInput {
            operation_id: OperationId::new(),
            subject,
            content: nous_memory::SchemaContent {
                producer: None,
                title: Some("Temporary operating rule".into()),
                structural_claim:
                    "Continue an open task after explicitly selecting its durable context".into(),
                applicability_scope: SchemaScope {
                    description: "Open development tasks".into(),
                    aboutness: vec![],
                    tags: vec![],
                    valid_time: Default::default(),
                },
                boundary_definition: "Does not authorize resuming an ended task".into(),
                formation_kind: SchemaFormationKind::ExplicitImport,
                evidence_links: vec![SchemaEvidenceLinkInput {
                    role: SchemaEvidenceRole::Support,
                    basis: RevisionBasis::Evidence(EvidenceRef {
                        epistemic_relation: None,
                        occurrence_id: source.occurrence.occurrence_id,
                        locator: EvidenceLocator::WholeOccurrence,
                        basis_role: BasisRole::Direct,
                    }),
                }],
            },
        })
        .await
        .unwrap();
    let schema_id = schema.schema.schema_id;
    let revision_id = schema.revision.schema_revision_id;
    let exact = || {
        let mut request = query(subject);
        request.projection.domains = vec![nous_core::ResultDomain::Schema];
        request.expression.targets = vec![QueryTarget::Exact {
            reference: CognitiveRef::CognitiveSchemaRevision(revision_id),
        }];
        request
    };
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    let use_input = UseFeedback {
        subject,
        session_id: None,
        consumer_ref: "consumer:test:schema-cleanup".into(),
        events: vec![UseFeedbackEvent {
            query_id: None,
            event_id: UseEventId::new(),
            reference: CognitiveRef::CognitiveSchemaRevision(revision_id),
            use_kind: UseKind::Referenced,
            occurred_at: Utc::now(),
            context: serde_json::json!({}),
        }],
    };
    assert_eq!(
        runtime
            .cognition
            .use_feedback(use_input.clone())
            .await
            .unwrap()
            .0,
        1
    );
    let suppress_operation = OperationId::new();
    let suppressed = owner
        .mutate_schema_lifecycle(subject, schema_id, suppress_operation, 1, Action::Suppress)
        .await
        .unwrap();
    assert_eq!(suppressed.schema.object_epoch, 2);
    assert_eq!(suppressed.revision.schema_revision_id, revision_id);
    assert!(runtime.query(exact()).await.unwrap().results.is_empty());
    assert!(matches!(
        owner
            .mutate_schema_lifecycle(subject, schema_id, OperationId::new(), 1, Action::Restore)
            .await,
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::StaleRevision
    ));
    owner
        .mutate_schema_lifecycle(subject, schema_id, OperationId::new(), 2, Action::Restore)
        .await
        .unwrap();
    let replay = owner
        .mutate_schema_lifecycle(subject, schema_id, suppress_operation, 1, Action::Suppress)
        .await
        .unwrap();
    assert_eq!(replay.schema.object_epoch, 3);
    assert_eq!(
        replay.schema.suppression_state,
        nous_core::SuppressionState::Normal
    );
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    owner
        .mutate_schema_lifecycle(subject, schema_id, OperationId::new(), 3, Action::Withdraw)
        .await
        .unwrap();
    assert!(runtime.query(exact()).await.unwrap().results.is_empty());
    owner
        .mutate_schema_lifecycle(subject, schema_id, OperationId::new(), 4, Action::Reaccept)
        .await
        .unwrap();
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    let purge_operation = OperationId::new();
    owner
        .purge_schema(subject, schema_id, purge_operation, 5)
        .await
        .unwrap();
    let duplicate = runtime.cognition.use_feedback(use_input).await.unwrap();
    assert_eq!((duplicate.0, duplicate.1), (0, 1));
    owner
        .purge_schema(subject, schema_id, purge_operation, 5)
        .await
        .unwrap();
    assert!(matches!(
        owner.schema_revision(subject, revision_id).await,
        Err(nous_core::Error::NotFound(_))
    ));
    assert!(matches!(
        owner
            .mutate_schema_lifecycle(subject, schema_id, suppress_operation, 1, Action::Suppress)
            .await,
        Err(nous_core::Error::NotFound(_))
    ));
    assert!(
        runtime
            .material
            .artifact(subject, source.artifact.as_ref().unwrap().artifact_id)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn synthesized_schema_requires_independent_known_roots() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let first = observation(&runtime, subject, "same root").await;
    let second = observation(&runtime, subject, "independent root").await;
    let link = |occurrence| SchemaEvidenceLinkInput {
        role: SchemaEvidenceRole::Support,
        basis: RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: None,
            occurrence_id: occurrence,
            locator: EvidenceLocator::WholeOccurrence,
            basis_role: BasisRole::Direct,
        }),
    };
    let base = || SchemaScope {
        description: "cognitive retrieval schema".into(),
        aboutness: Vec::new(),
        tags: Vec::new(),
        valid_time: Default::default(),
    };
    let same_root = runtime
        .require_memory()
        .unwrap()
        .create_schema(CreateSchemaInput {
            operation_id: OperationId::new(),
            subject,
            content: nous_memory::SchemaContent {
                producer: None,
                title: None,
                structural_claim: "same root must reject".into(),
                applicability_scope: base(),
                boundary_definition: "none".into(),
                formation_kind: SchemaFormationKind::Synthesized,
                evidence_links: vec![
                    link(first.occurrence.occurrence_id),
                    link(first.occurrence.occurrence_id),
                ],
            },
        })
        .await;
    assert!(matches!(same_root, Err(nous_core::Error::Invalid(_))));
    let independent = runtime
        .require_memory()
        .unwrap()
        .create_schema(CreateSchemaInput {
            operation_id: OperationId::new(),
            subject,
            content: nous_memory::SchemaContent {
                producer: None,
                title: None,
                structural_claim: "independent roots accept".into(),
                applicability_scope: base(),
                boundary_definition: "none".into(),
                formation_kind: SchemaFormationKind::Synthesized,
                evidence_links: vec![
                    link(first.occurrence.occurrence_id),
                    link(second.occurrence.occurrence_id),
                ],
            },
        })
        .await;
    assert!(independent.is_ok());
}

#[tokio::test]
async fn memory_revision_identity_guard_rejects_disjoint_aboutness() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let observation = observation(&runtime, subject, "identity guard").await;
    let alice = EntityRef::new("entity:alice").expect("alice");
    let bob = EntityRef::new("entity:bob").expect("bob");
    let mut input = form_input(
        subject,
        observation.occurrence.occurrence_id,
        OperationId::new(),
        "Alice identity matter",
    );
    input.aboutness = vec![alice.clone()];
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(input)
        .await
        .expect("memory");
    let mut revision = form_input(
        subject,
        observation.occurrence.occurrence_id,
        OperationId::new(),
        "Bob disjoint matter",
    );
    revision.aboutness = vec![bob.clone()];
    let rejected = runtime
        .require_memory()
        .unwrap()
        .revise_memory(nous_memory::ReviseMemoryInput {
            producer: None,
            operation_id: revision.operation_id,
            subject,
            memory_id: memory.object.memory_id,
            expected_object_epoch: memory.object.object_epoch,
            intent: nous_memory::RevisionIntent::Correct,
            formation_mode: revision.formation_mode,
            grounding_occurrence_id: revision.grounding_occurrence_id,
            semantic_role: revision.semantic_role,
            representation_text: revision.representation_text,
            title: revision.title,
            basis: revision.basis,
            aboutness: revision.aboutness,
            valid_time: revision.valid_time,

            epistemic_class: revision.epistemic_class,
        })
        .await;
    assert!(matches!(
        rejected,
        Err(nous_core::Error::FailedPrecondition(_))
    ));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "association contract keeps invalid and valid producer paths together"
)]
async fn association_requires_exact_cognition_and_valid_basis_class() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, false, false, true).await;
    let subject = subject(&runtime).await;
    let entity = EntityRef::new("entity:association").expect("entity");
    let observation = observation(&runtime, subject, "association evidence").await;
    let mut input = form_input(
        subject,
        observation.occurrence.occurrence_id,
        OperationId::new(),
        "association memory",
    );
    input.aboutness = vec![entity.clone()];
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(input)
        .await
        .expect("memory");
    let invalid = runtime
        .require_memory()
        .unwrap()
        .create_association(
            nous_memory::CreateAssociationRequest {
                producer: None,
                operation_id: OperationId::new(),
                from: CognitiveRef::Memory(memory.object.memory_id),
                to: CognitiveRef::Entity(entity.clone()),
                relation_kind: "assoc.related".into(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::HostExplicit,
                basis: vec![AssociationBasis::Revision(RevisionBasis::Evidence(
                    EvidenceRef {
                        epistemic_relation: None,
                        occurrence_id: observation.occurrence.occurrence_id,
                        locator: EvidenceLocator::WholeOccurrence,
                        basis_role: BasisRole::Direct,
                    },
                ))],
                producer_signature_id: None,
                valid_time: Default::default(),
            },
            subject,
        )
        .await;
    assert!(matches!(invalid, Err(nous_core::Error::Invalid(_))));
    let event_id = UseEventId::new();
    runtime
        .cognition
        .use_feedback(UseFeedback {
            subject,
            session_id: None,
            consumer_ref: "consumer:test:association".into(),
            events: vec![UseFeedbackEvent {
                query_id: None,
                event_id,
                reference: CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
                use_kind: UseKind::Referenced,
                occurred_at: Utc::now(),
                context: serde_json::json!({"association":"test"}),
            }],
        })
        .await
        .expect("use event");
    let association = runtime
        .require_memory()
        .unwrap()
        .create_association(
            nous_memory::CreateAssociationRequest {
                producer: None,
                operation_id: OperationId::new(),
                from: CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
                to: CognitiveRef::Entity(entity.clone()),
                relation_kind: "assoc.related".into(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::MeaningfulUse,
                basis: vec![AssociationBasis::UseEvent(UseEventRef {
                    subject_id: subject,
                    consumer_ref: "consumer:test:association".into(),
                    event_id,
                })],
                producer_signature_id: None,
                valid_time: Default::default(),
            },
            subject,
        )
        .await
        .expect("meaningful-use association");
    assert_eq!(association.basis.len(), 1);
    let topology = runtime
        .serving
        .projection_input(
            subject,
            true,
            runtime
                .configuration
                .snapshot_for_subject(subject)
                .unwrap()
                .get(nous_memory::EPISODE_SYNOPSIS)
                .unwrap(),
            None,
        )
        .await
        .expect("topology input");
    assert!(topology.topology.edges.iter().any(|edge| {
        edge.association_kind == "assoc.related" && edge.provenance_root.is_none()
    }));
    let projection_budget = nous_persistence::EpisodeTextBudget {
        max_members: 32,
        fragment_max_bytes: 4096,
        total_max_bytes: 16384,
    };
    let cognitive = runtime
        .serving
        .projection_input(subject, true, projection_budget, None)
        .await
        .expect("coherent cognitive projection");
    assert_eq!(cognitive.authority_watermark, topology.authority_watermark);
    assert_eq!(cognitive.topology.nodes, topology.topology.nodes);
    let memory_reference = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    assert!(
        cognitive
            .sources
            .iter()
            .any(|s| s.reference == memory_reference && s.text.is_some())
    );
    assert!(
        cognitive
            .topology
            .edges
            .iter()
            .any(|e| e.association_kind == "assoc.related" && e.provenance_root.is_none())
    );
    let forbidden = runtime
        .serving
        .projection_input(subject, false, projection_budget, None)
        .await
        .expect("cognitive projection without memory capability");
    assert!(
        !forbidden
            .sources
            .iter()
            .any(|s| s.reference == memory_reference)
    );
    assert!(!forbidden.topology.nodes.contains(&memory_reference));
    let memory_roots = &cognitive.evidence_roots[&memory_reference];
    assert!(!memory_roots.is_empty());
    assert!(
        memory_roots
            .iter()
            .any(|root| !root.starts_with("unknown-dependency:"))
    );
    assert!(!forbidden.evidence_roots.contains_key(&memory_reference));
}
