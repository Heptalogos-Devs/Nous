// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "concept_maintenance/catalog.rs"]
mod catalog;
#[path = "concept_maintenance/query.rs"]
mod query;
mod test_support;
use nous_core::*;
use nous_kernel::NousRuntime;
use nous_memory::*;
use nous_runtime::{CognitiveProfile, MaintenanceDisposition, MaintenanceNeed};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use test_support::*;
fn producer() -> ProducerSignature {
    ProducerSignature {
        model_role: None,
        model_profile: None,
        execution_profile: None,
        inference_controls_digest: None,
        role_policy_digest: None,
        prompt_id: None,
        prompt_digest: None,

        signature_hash: String::new(),
        provider_class: "deterministic-fixture".into(),
        operation: CapabilityOperation::ConceptMaintenanceText,
        implementation: "topology-fixture".into(),
        model_identity: Some("scope-aware-fixture".into()),
        model_revision: None,
        output_schema_digest: Some("a".repeat(64)),
        preprocessing_identity: "program/memory/concept-maintenance.md".into(),
        preprocessing_revision: "fixture-v1".into(),
        config_digest: "b".repeat(64),
    }
}
async fn claim(rt: &NousRuntime, subject: SubjectId) -> MaintenanceNeed {
    rt.cognition
        .lease_maintenance(subject, &["concept_maintenance".into()], 1, 120, None)
        .await
        .unwrap()
        .pop()
        .unwrap()
}
async fn assert_subject_producer(rt: &NousRuntime, subject: SubjectId, implementation: &str) {
    use nous_protocol::nous::wave::v1alpha1 as p;
    use p::material_service_server::MaterialService;
    let producer: uuid::Uuid = sqlx::query_scalar(
        "SELECT producer_signature_id FROM producer_signatures WHERE implementation=$1",
    )
    .bind(implementation)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    let service = nous_kernel::transport::KernelService(rt.clone());
    let request = |subject: SubjectId| {
        tonic::Request::new(p::ObjectRequest {
            subject_id: subject.0.to_string(),
            id: producer.to_string(),
        })
    };
    let found = service
        .get_producer(request(subject))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(found.implementation, implementation);
    let foreign = service
        .get_producer(request(SubjectId::new()))
        .await
        .unwrap_err();
    assert_eq!(foreign.code(), tonic::Code::NotFound);
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one live owner and Serving scenario verifies local plans, derived signals and real policy effects"
)]
async fn concepts_use_local_typed_catalogs_and_derived_accretion_without_batch_commit() {
    let (root, url, _postgres) = database().await;
    let rt = open_runtime_with_serving(&url, &root, true, false, true).await;
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
    let entity = CognitiveRef::Entity(EntityRef::new("entity:fixture:release-engineer").unwrap());
    assert!(
        !rt.store
            .reference_in_subject(subject, &entity)
            .await
            .unwrap()
    );
    rt.store
        .bind_identity(
            subject,
            entity.clone(),
            "Alice".into(),
            vec!["Release engineer".into()],
        )
        .await
        .unwrap();
    assert!(
        rt.store
            .reference_in_subject(subject, &entity)
            .await
            .unwrap()
    );
    assert!(
        !rt.store
            .reference_in_subject(SubjectId::new(), &entity)
            .await
            .unwrap()
    );
    let mut revisions = Vec::new();
    for text in [
        "Alice requires recorded reviewer approval before deployment",
        "Bob requires recorded reviewer approval before releasing the service",
    ] {
        let occurrence = observation(&rt, subject, text).await;
        let view = owner
            .form_memory(form_input(
                subject,
                occurrence.occurrence.occurrence_id,
                OperationId::new(),
                text,
            ))
            .await
            .unwrap();
        revisions.push(CognitiveRef::MemoryRevision(
            view.revision.memory_revision_id,
        ));
    }
    let first = claim(&rt, subject).await;
    catalog::assert_wire_catalog(&rt, subject, &revisions, &first).await;
    let initial = owner
        .plan_concepts(subject, revisions[0].clone())
        .await
        .unwrap();
    assert_eq!(
        initial.references.len(),
        1,
        "unrelated Subject cognition is absent"
    );
    assert!(initial.tags.is_empty());
    let model = initial.model_input.to_string();
    assert!(!model.contains(&subject.0.to_string()));
    assert!(!model.contains(&reference_parts(&revisions[0]).1));
    assert!(model.contains("exact_cognition"));
    let lexical = owner
        .create_tag(
            subject,
            CreateTagRequest {
                producer: Some(producer()),
                operation_id: OperationId::new(),
                label: "Reviewer approval".into(),
                description: None,
                kind_hint: None,
                origin: "concept_maintenance".into(),
            },
        )
        .await
        .unwrap();
    let candidates = owner
        .plan_concepts(subject, revisions[1].clone())
        .await
        .unwrap();
    assert_eq!(
        candidates.tags.len(),
        1,
        "partial name words produce a reuse candidate"
    );
    assert_eq!(candidates.tags[0].target.tag_id, lexical.tag_id);
    assert!(!candidates.tags[0].attached);
    let concept = lexical;
    assert_subject_producer(&rt, subject, "topology-fixture").await;
    let tag = CognitiveRef::Tag(concept.tag_id);
    let mut association_producer = producer();
    association_producer.implementation = "association-only-fixture".into();
    for reference in &revisions {
        owner
            .create_association(
                CreateAssociationRequest {
                    producer: Some(association_producer.clone()),
                    operation_id: OperationId::new(),
                    from: reference.clone(),
                    to: tag.clone(),
                    relation_kind: "tag_attachment".into(),
                    polarity: AssociationPolarity::Positive,
                    basis_class: AssociationBasisClass::CognitiveDerivation,
                    basis: vec![AssociationBasis::Revision(
                        RevisionBasis::CognitionDependency(CognitionDependency {
                            epistemic_relation: None,
                            target_revision: reference.clone(),
                            basis_role: BasisRole::Direct,
                        }),
                    )],
                    producer_signature_id: None,
                    valid_time: TemporalExtent::Unknown,
                },
                subject,
            )
            .await
            .unwrap();
    }
    assert_subject_producer(&rt, subject, "association-only-fixture").await;
    owner
        .create_association(
            CreateAssociationRequest {
                producer: Some(producer()),
                operation_id: OperationId::new(),
                from: revisions[0].clone(),
                to: revisions[1].clone(),
                relation_kind: "assoc.related".into(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::CognitiveDerivation,
                basis: vec![AssociationBasis::Revision(
                    RevisionBasis::CognitionDependency(CognitionDependency {
                        epistemic_relation: None,
                        target_revision: revisions[0].clone(),
                        basis_role: BasisRole::Direct,
                    }),
                )],
                producer_signature_id: None,
                valid_time: TemporalExtent::Unknown,
            },
            subject,
        )
        .await
        .unwrap();
    let local = owner
        .plan_concepts(subject, revisions[0].clone())
        .await
        .unwrap();
    assert_eq!(
        local
            .references
            .values()
            .filter(|r| matches!(r, CognitiveRef::MemoryRevision(_)))
            .count(),
        2
    );
    assert_eq!(local.tags.len(), 1);
    assert!(local.tags[0].attached);
    rt.cognition
        .acknowledge_maintenance(&first, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let second = claim(&rt, subject).await;
    rt.cognition
        .acknowledge_maintenance(&second, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let historical_tags: i64 =
        sqlx::query_scalar("SELECT count(*) FROM memory_revision_tags WHERE memory_revision_id=$1")
            .bind(match revisions[0] {
                CognitiveRef::MemoryRevision(id) => id.0,
                _ => unreachable!(),
            })
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    assert_eq!(historical_tags, 0);
    episode_relations(&rt, subject, &revisions).await;
    let episode_need = claim(&rt, subject).await;
    rt.cognition
        .acknowledge_maintenance(&episode_need, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let before = owner
        .accretion_signal(subject, &tag)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.attached_cognition, 2);
    assert_eq!(before.independent_roots, 2);
    assert_eq!(before.meaningful_use, 0);
    assert_eq!(before.cross_episode_recurrence, 1);
    let policy = rt
        .configuration
        .snapshot_for_subject(subject)
        .unwrap()
        .get(ACCRETION)
        .unwrap();
    assert_eq!(before.review_priority(&policy), 20);
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            ACCRETION.path(),
            serde_json::json!({"enabled":true,"generic_degree":32,"recurrence_review":1}),
            None,
        )
        .await
        .unwrap();
    let reviewed = owner
        .plan_concepts(subject, revisions[0].clone())
        .await
        .unwrap();
    assert_eq!(
        reviewed.model_input["tags"][0]["accretion"]["reviewPriority"],
        30
    );
    assert_ne!(reviewed.config_digest, local.config_digest);
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            ACCRETION.path(),
            serde_json::json!({"enabled":true,"generic_degree":32,"recurrence_review":3}),
            None,
        )
        .await
        .unwrap();

    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let events = (0..3)
        .map(|_| nous_runtime::UseFeedbackEvent {
            query_id: None,
            event_id: UseEventId::new(),
            reference: revisions[0].clone(),
            use_kind: nous_runtime::UseKind::ActedOn,
            occurred_at: rt.cognition.now(subject),
            context: serde_json::json!({}),
        })
        .collect();
    rt.cognition
        .use_feedback(nous_runtime::UseFeedback {
            subject,
            session_id: Some(session.session_id),
            consumer_ref: "consumer:accretion:fixture".into(),
            events,
        })
        .await
        .unwrap();
    let after = owner
        .accretion_signal(subject, &tag)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.meaningful_use, 3);
    assert_eq!(after.independent_roots, before.independent_roots);
    let truth: String = sqlx::query_scalar(
        "SELECT epistemic_class FROM memory_revisions WHERE memory_revision_id=$1",
    )
    .bind(match revisions[0] {
        CognitiveRef::MemoryRevision(id) => id.0,
        _ => unreachable!(),
    })
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(truth, "observed");
    let queued:i64=sqlx::query_scalar("SELECT count(*) FROM maintenance_needs WHERE subject_id=$1 AND kind='concept_maintenance' AND state='pending'").bind(subject.0).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(queued, 1);
    let review = claim(&rt, subject).await;
    rt.cognition
        .acknowledge_maintenance(&review, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    rt.cognition
        .use_feedback(nous_runtime::UseFeedback {
            subject,
            session_id: Some(session.session_id),
            consumer_ref: "consumer:accretion:fixture".into(),
            events: [
                nous_runtime::UseKind::Presented,
                nous_runtime::UseKind::ResultRefuted,
            ]
            .into_iter()
            .map(|use_kind| nous_runtime::UseFeedbackEvent {
                query_id: None,
                event_id: UseEventId::new(),
                reference: revisions[0].clone(),
                use_kind,
                occurred_at: rt.cognition.now(subject),
                context: serde_json::json!({}),
            })
            .collect(),
        })
        .await
        .unwrap();
    let negative = owner
        .accretion_signal(subject, &tag)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(negative.meaningful_use, 3);
    assert_eq!(negative.counterevidence, 1);
    let queued:i64=sqlx::query_scalar("SELECT count(*) FROM maintenance_needs WHERE subject_id=$1 AND kind='concept_maintenance' AND state='pending'").bind(subject.0).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(
        queued, 0,
        "exposure/refutation cannot cross a positive use interval"
    );
    query::assert_maintained_concept_query(&rt, subject, &tag, &revisions).await;
    catalog::assert_schema_accretion(&rt, subject, &revisions).await;

    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            ACCRETION.path(),
            serde_json::json!({"enabled":false,"generic_degree":32,"recurrence_review":1}),
            None,
        )
        .await
        .unwrap();
    assert!(
        owner
            .accretion_signal(subject, &tag)
            .await
            .unwrap()
            .is_none()
    );
    let disabled = owner
        .plan_concepts(subject, revisions[0].clone())
        .await
        .unwrap();
    assert_eq!(disabled.tags.len(), 1);
    assert_eq!(disabled.associations.len(), 2);
    assert!(disabled.tags[0].accretion.is_none());
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            CONCEPT_MAINTENANCE.path(),
            serde_json::json!({"max_candidates":1,"max_suggestions":2}),
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        owner
            .plan_concepts(subject, revisions[0].clone())
            .await
            .unwrap()
            .policy
            .max_suggestions,
        2
    );
}
async fn episode_relations(rt: &NousRuntime, subject: SubjectId, revisions: &[CognitiveRef]) {
    let owner = rt.require_memory().unwrap();
    let episode = owner
        .create_episode(EpisodeInput {
            operation_id: OperationId::new(),
            subject,
            track_key: "relation-witness".into(),
            title: Some("Recorded approval sequence".into()),
            parent_episode_revision_id: None,
            experience_time: TemporalExtent::Unknown,
            boundary_explanation: "Both exact members recorded in order".into(),
            producer_signature_id: None,
            members: revisions
                .iter()
                .enumerate()
                .map(|(index, reference)| EpisodeMemberInput {
                    reference: reference.clone(),
                    role: if index == 0 { "procedure" } else { "outcome" }.into(),
                })
                .collect(),
            basis: revisions
                .iter()
                .map(|reference| {
                    RevisionBasis::CognitionDependency(CognitionDependency {
                        epistemic_relation: None,
                        target_revision: reference.clone(),
                        basis_role: BasisRole::Direct,
                    })
                })
                .collect(),
        })
        .await
        .unwrap();
    let projection = rt
        .store
        .topology_projection_input(subject, true)
        .await
        .unwrap();
    let episode_ref = CognitiveRef::EpisodeRevision(episode.revision.episode_revision_id);
    assert!(projection.nodes.contains(&episode_ref));
    for reference in revisions {
        assert!(projection.edges.iter().any(|edge| edge.from == episode_ref
            && &edge.to == reference
            && edge.association_kind == "cognition_basis"));
        assert!(projection.edges.iter().any(|edge| &edge.from == reference
            && edge.to == episode_ref
            && edge.association_kind == "cognition_basis"));
    }
    assert!(
        !rt.store
            .topology_projection_input(subject, false)
            .await
            .unwrap()
            .nodes
            .contains(&episode_ref)
    );
}
