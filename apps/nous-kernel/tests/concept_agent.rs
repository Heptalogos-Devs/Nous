// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;
use nous_core::*;
use nous_kernel::transport::KernelService;
use nous_memory::{
    AssociationBasis, AssociationBasisClass, AssociationPolarity, CreateAssociationRequest,
    CreateTagRequest,
};
use nous_protocol::nous::wave::v1alpha1 as p;
use nous_subject::{CognitiveSeedInput, CreateSubject};
use p::concept_service_server::ConceptService;
use test_support::*;
use tonic::Request;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one Subject scenario verifies paging and supported graph lifecycle"
)]
async fn agent_concept_reads_supported_edges_and_searches_the_tag_catalog() {
    let (root, url, _postgres) = database().await;
    let rt = open_runtime(&url, &root).await;
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
    let memory = rt.require_memory().unwrap();
    let producer = ProducerSignature {
        model_role: None,
        model_profile: None,
        execution_profile: None,
        inference_controls_digest: None,
        role_policy_digest: None,
        prompt_id: None,
        prompt_digest: None,

        signature_hash: String::new(),
        provider_class: "deterministic_test".into(),
        operation: CapabilityOperation::ConceptMaintenanceText,
        implementation: "concept-proposal-test".into(),
        model_identity: None,
        model_revision: None,
        output_schema_digest: None,
        preprocessing_identity: "concept-maintenance".into(),
        preprocessing_revision: "1".into(),
        config_digest: "test-config".into(),
    };
    let mut tags = Vec::new();
    for label in ["Deployment approval", "Deployment rollout", "Garden"] {
        let tag = memory
            .create_tag(
                subject,
                CreateTagRequest {
                    producer: Some(producer.clone()),
                    operation_id: OperationId::new(),
                    label: label.into(),
                    description: None,
                    kind_hint: Some("topic".into()),
                    origin: "concept_maintenance".into(),
                },
            )
            .await
            .unwrap();
        tags.push(tag.tag_id);
    }
    rt.store
        .bind_identity(
            subject,
            CognitiveRef::Tag(tags[2]),
            "Garden".into(),
            vec!["deployment-garden".into()],
        )
        .await
        .unwrap();
    let observed = observation(
        &rt,
        subject,
        "Deployment approval precedes rollout; rollout includes the Garden environment",
    )
    .await;
    let basis = AssociationBasis::Revision(RevisionBasis::Evidence(EvidenceRef {
        epistemic_relation: None,
        occurrence_id: observed.occurrence.occurrence_id,
        locator: EvidenceLocator::WholeOccurrence,
        basis_role: BasisRole::Direct,
    }));
    let mut edge_ids = Vec::new();
    let mut requests = Vec::new();
    for (from, to) in [(tags[0], tags[1]), (tags[1], tags[2])] {
        let request = CreateAssociationRequest {
            producer: Some(producer.clone()),
            operation_id: OperationId::new(),
            from: CognitiveRef::Tag(from),
            to: CognitiveRef::Tag(to),
            relation_kind: "assoc.related".into(),
            polarity: AssociationPolarity::Positive,
            basis_class: AssociationBasisClass::CognitiveDerivation,
            basis: vec![basis.clone()],
            producer_signature_id: None,
            valid_time: TemporalExtent::Unknown,
        };
        let edge = memory
            .create_association(request.clone(), subject)
            .await
            .unwrap();
        let replay = memory
            .create_association(request.clone(), subject)
            .await
            .unwrap();
        assert_eq!(edge.association_evidence_id, replay.association_evidence_id);
        requests.push(request.clone());
        let mut invalid = request;
        invalid.operation_id = OperationId::new();
        invalid.relation_kind = "assoc.invented".into();
        assert!(matches!(
            memory.create_association(invalid, subject).await,
            Err(Error::Invalid(_))
        ));
        edge_ids.push(edge.association_evidence_id);
    }
    let producers: i64 = sqlx::query_scalar("SELECT count(DISTINCT producer_signature_id) FROM tag_revisions WHERE tag_id=ANY($1::uuid[])")
        .bind(tags.iter().map(|tag| tag.0).collect::<Vec<_>>()).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(producers, 1);
    let association_producers: i64 = sqlx::query_scalar("SELECT count(DISTINCT producer_signature_id) FROM association_evidence WHERE subject_id=$1")
        .bind(subject.0).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(association_producers, 1);
    let service = KernelService(rt.clone());
    let entity = EntityRef::new("entity:release-reviewer").unwrap();
    rt.store
        .bind_identity(
            subject,
            CognitiveRef::Entity(entity.clone()),
            "Release reviewer".into(),
            Vec::new(),
        )
        .await
        .unwrap();
    let mention = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO entity_mentions(mention_id,subject_id,occurrence_id,surface,created_at) VALUES($1,$2,$3,$4,$5)")
        .bind(mention).bind(subject.0).bind(observed.occurrence.occurrence_id.0).bind("Release reviewer").bind(rt.cognition.now(subject)).execute(rt.store.pool()).await.unwrap();
    let binding = p::RebindEntityRequest {
        subject_id: subject.0.to_string(),
        mention_id: mention.to_string(),
        entity_ref: Some(entity.as_str().into()),
        binding_state: "bound".into(),
        host_resolution_ref: None,
        reason: Some("Host resolved source mention".into()),
    };
    p::identity_service_server::IdentityService::rebind_entity(
        &service,
        Request::new(binding.clone()),
    )
    .await
    .unwrap();
    let mut foreign = binding;
    foreign.subject_id = SubjectId::new().0.to_string();
    assert!(
        p::identity_service_server::IdentityService::rebind_entity(&service, Request::new(foreign))
            .await
            .is_err()
    );
    let revision_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM entity_binding_revisions WHERE mention_id=$1")
            .bind(mention)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    assert_eq!(revision_count, 1);

    let search = |text: &str, token: &str| p::SearchTagsRequest {
        subject_id: subject.0.to_string(),
        text: text.into(),
        page: Some(p::Page {
            page_size: 1,
            page_token: token.into(),
        }),
    };
    let first = ConceptService::search_tags(&service, Request::new(search("Deployment", "")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.items.len(), 1);
    assert!(!first.next_page_token.is_empty());
    let second = ConceptService::search_tags(
        &service,
        Request::new(search("Deployment", &first.next_page_token)),
    )
    .await
    .unwrap()
    .into_inner();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].tag_id, second.items[0].tag_id);
    let third = ConceptService::search_tags(
        &service,
        Request::new(search("Deployment", &second.next_page_token)),
    )
    .await
    .unwrap()
    .into_inner();
    assert_eq!(third.items[0].tag_id, tags[2].0.to_string());
    assert!(third.next_page_token.is_empty());
    assert!(
        ConceptService::search_tags(
            &service,
            Request::new(search("Garden", &first.next_page_token))
        )
        .await
        .is_err()
    );
    let request = |nodes, depth| p::NeighborhoodRequest {
        subject_id: subject.0.to_string(),
        root: Some(p::CognitiveRef {
            kind: "tag".into(),
            value: tags[0].0.to_string(),
        }),
        max_nodes: nodes,
        max_depth: depth,
    };
    let one = ConceptService::get_neighborhood(&service, Request::new(request(64, 1)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(one.nodes.len(), 2);
    assert_eq!(one.associations.len(), 1);
    assert_eq!(one.associations[0].basis.len(), 1);
    let two = ConceptService::get_neighborhood(&service, Request::new(request(64, 2)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(two.nodes.len(), 3);
    assert_eq!(two.associations.len(), 2);
    let limited = ConceptService::get_neighborhood(&service, Request::new(request(1, 2)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(limited.nodes.len(), 1);
    assert!(limited.truncated);
    memory
        .revoke_association(subject, edge_ids[0], OperationId::new())
        .await
        .unwrap();
    let revoked = ConceptService::get_neighborhood(&service, Request::new(request(64, 2)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(revoked.nodes.len(), 1);
    assert!(revoked.associations.is_empty());
    let store = &rt.store;
    let revision = |tag: TagId| async move {
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT current_revision_id FROM tags WHERE tag_id=$1")
            .bind(tag.0)
            .fetch_one(store.pool())
            .await
            .unwrap()
    };
    memory
        .merge_tags(
            subject,
            nous_memory::MergeTagsInput {
                operation_id: OperationId::new(),
                survivor: nous_memory::TagExpectation {
                    tag_id: tags[2],
                    expected_revision_id: revision(tags[2]).await,
                },
                retired: vec![nous_memory::TagExpectation {
                    tag_id: tags[1],
                    expected_revision_id: revision(tags[1]).await,
                }],
                basis: vec![match basis {
                    AssociationBasis::Revision(basis) => basis,
                    _ => unreachable!(),
                }],
            },
        )
        .await
        .unwrap();
    let replay = memory
        .create_association(requests[1].clone(), subject)
        .await
        .unwrap();
    assert_eq!(replay.association_evidence_id, edge_ids[1]);
    let mut stale = requests[1].clone();
    stale.operation_id = OperationId::new();
    assert!(matches!(
        memory.create_association(stale, subject).await,
        Err(Error::Domain(error)) if error.code == nous_core::DomainErrorCode::StaleRevision
    ));
}
