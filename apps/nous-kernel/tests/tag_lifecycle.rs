// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;
use nous_core::*;
use nous_kernel::transport::KernelService;
use nous_memory::{
    AssociationBasis, AssociationBasisClass, AssociationPolarity, CreateAssociationRequest,
    CreateTagRequest, MergeTagsInput, SplitTagInput, TagContent, TagExpectation,
};
use nous_protocol::nous::wave::v1alpha1 as p;
use nous_subject::{CognitiveSeedInput, CreateSubject};
use p::concept_service_server::ConceptService;
use test_support::*;
use tonic::Request;

fn content(label: &str) -> TagContent {
    TagContent {
        label: label.into(),
        description: Some("Release process concept".into()),
        kind_hint: Some("procedure".into()),
    }
}
fn expectation(tag: &nous_memory::Tag) -> TagExpectation {
    TagExpectation {
        tag_id: tag.tag_id,
        expected_revision_id: tag.current_revision_id,
    }
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one lifecycle scenario verifies history, aliases, merge interpretation and split scope"
)]
async fn concept_lineage_preserves_history_and_canonicalizes_current_query_and_serving() {
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
    let owner = rt.require_memory().unwrap();
    let mut tags = Vec::new();
    for label in [
        "Deployment approval",
        "Release authorization",
        "Release process",
    ] {
        tags.push(
            owner
                .create_tag(
                    subject,
                    CreateTagRequest {
                        producer: None,
                        operation_id: OperationId::new(),
                        label: label.into(),
                        description: None,
                        kind_hint: None,
                        origin: "host_explicit".into(),
                    },
                )
                .await
                .unwrap(),
        );
    }
    let (_, old_binding) = rt
        .store
        .resolve_identity(subject, "tag", "Deployment approval", false)
        .await
        .unwrap();
    let old_lexical = old_binding[0].lexical_ref.clone();
    let service = KernelService(rt.clone());
    let revise = p::ReviseTagRequest {
        producer: None,
        operation_id: OperationId::new().0.to_string(),
        subject_id: subject.0.to_string(),
        target: Some(p::TagRevisionTarget {
            tag_id: tags[0].tag_id.0.to_string(),
            expected_revision_id: tags[0].current_revision_id.to_string(),
        }),
        content: Some(p::TagContent {
            label: "Release approval".into(),
            description: Some("Approval before rollout".into()),
            kind_hint: Some("procedure".into()),
        }),
    };
    let revised = ConceptService::revise_tag(&service, Request::new(revise.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_ne!(
        revised.current_revision_id,
        tags[0].current_revision_id.to_string()
    );
    let replay = ConceptService::revise_tag(&service, Request::new(revise.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(revised.current_revision_id, replay.current_revision_id);
    let later = p::ReviseTagRequest {
        operation_id: OperationId::new().0.to_string(),
        target: Some(p::TagRevisionTarget {
            tag_id: revised.tag_id.clone(),
            expected_revision_id: revised.current_revision_id.clone(),
        }),
        content: Some(p::TagContent {
            label: "Release approval".into(),
            description: Some("Approval before a controlled rollout".into()),
            kind_hint: Some("procedure".into()),
        }),
        ..revise.clone()
    };
    let later_revision = ConceptService::revise_tag(&service, Request::new(later))
        .await
        .unwrap()
        .into_inner();
    let exact_replay = ConceptService::revise_tag(&service, Request::new(revise.clone()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        exact_replay.current_revision_id,
        revised.current_revision_id
    );
    assert_eq!(exact_replay.description, revised.description);
    let mut stale = revise.clone();
    stale.operation_id = OperationId::new().0.to_string();
    assert_eq!(
        ConceptService::revise_tag(&service, Request::new(stale))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::Aborted
    );
    tags[0].current_revision_id = later_revision.current_revision_id.parse().unwrap();
    for alias in ["Deployment approval", "Release approval"] {
        let (status, result) = rt
            .store
            .resolve_identity(subject, "tag", alias, false)
            .await
            .unwrap();
        assert_eq!(status, "BOUND");
        assert_eq!(result[0].canonical, CognitiveRef::Tag(tags[0].tag_id));
    }
    let mut revisions = Vec::new();
    for text in [
        "Alice approved the October release before rollout",
        "Bob approved the November release before rollout",
    ] {
        let occurrence = observation(&rt, subject, text).await;
        let mut input = form_input(
            subject,
            occurrence.occurrence.occurrence_id,
            OperationId::new(),
            text,
        );
        input.tags = vec![tags[1].tag_id];
        revisions.push(
            owner
                .form_memory(input)
                .await
                .unwrap()
                .revision
                .memory_revision_id,
        );
    }
    let basis = revisions
        .iter()
        .map(|id| {
            RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: CognitiveRef::MemoryRevision(*id),
                basis_role: BasisRole::Direct,
            })
        })
        .collect::<Vec<_>>();
    let association = owner
        .create_association(
            CreateAssociationRequest {
                producer: None,
                operation_id: OperationId::new(),
                from: CognitiveRef::MemoryRevision(revisions[0]),
                to: CognitiveRef::Tag(tags[1].tag_id),
                relation_kind: "tag_attachment".into(),
                polarity: AssociationPolarity::Positive,
                basis_class: AssociationBasisClass::HostExplicit,
                basis: vec![AssociationBasis::Revision(basis[0].clone())],
                producer_signature_id: None,
                valid_time: TemporalExtent::Unknown,
            },
            subject,
        )
        .await
        .unwrap();
    let merge = MergeTagsInput {
        operation_id: OperationId::new(),
        survivor: expectation(&tags[0]),
        retired: vec![expectation(&tags[1])],
        basis: vec![basis[0].clone()],
    };
    owner.merge_tags(subject, merge.clone()).await.unwrap();
    owner.merge_tags(subject, merge).await.unwrap();
    let occurrence = observation(&rt, subject, "Explicit concept formation after merge").await;
    let mut explicit = form_input(
        subject,
        occurrence.occurrence.occurrence_id,
        OperationId::new(),
        "Explicit concept formation after merge",
    );
    explicit.tags = vec![tags[1].tag_id, tags[0].tag_id, tags[1].tag_id];
    let formed = owner.form_memory(explicit.clone()).await.unwrap();
    assert_eq!(formed.tags, vec![tags[0].tag_id]);
    assert_eq!(
        owner
            .form_memory(explicit)
            .await
            .unwrap()
            .revision
            .memory_revision_id,
        formed.revision.memory_revision_id
    );
    let mut invalid = form_input(
        subject,
        occurrence.occurrence.occurrence_id,
        OperationId::new(),
        "Unknown concept",
    );
    invalid.tags = vec![TagId::new()];
    assert!(owner.form_memory(invalid).await.is_err());

    for name in [
        "Release authorization",
        "Release approval",
        "Deployment approval",
    ] {
        let (status, binding) = rt
            .store
            .resolve_identity(subject, "tag", name, false)
            .await
            .unwrap();
        assert_eq!(status, "BOUND");
        assert_eq!(binding[0].canonical, CognitiveRef::Tag(tags[0].tag_id));
        assert_eq!(binding[0].display_name, "Release approval");
    }
    let (_, binding) = rt
        .store
        .resolve_identity(subject, "tag", &old_lexical, true)
        .await
        .unwrap();
    assert_eq!(binding[0].canonical, CognitiveRef::Tag(tags[0].tag_id));
    let addresses = rt
        .store
        .identity_addresses(&[
            nous_persistence::IdentityAddressTarget {
                subject,
                reference: CognitiveRef::Tag(tags[1].tag_id),
            },
            nous_persistence::IdentityAddressTarget {
                subject,
                reference: CognitiveRef::Tag(tags[0].tag_id),
            },
        ])
        .await
        .unwrap();
    assert_eq!(addresses[0].status, "BOUND");
    assert_eq!(addresses[0].lexical_ref, addresses[1].lexical_ref);
    let catalog = owner
        .plan_concepts(subject, CognitiveRef::MemoryRevision(revisions[0]))
        .await
        .unwrap();
    assert!(
        catalog
            .tags
            .iter()
            .find(|t| t.target.tag_id == tags[0].tag_id)
            .unwrap()
            .aliases
            .contains(&"Release authorization".to_string())
    );
    let unchanged: uuid::Uuid =
        sqlx::query_scalar("SELECT tag_id FROM memory_revision_tags WHERE memory_revision_id=$1")
            .bind(revisions[0].0)
            .fetch_one(rt.store.pool())
            .await
            .unwrap();
    assert_eq!(unchanged, tags[1].tag_id.0);
    let evidence_endpoint: String = sqlx::query_scalar(
        "SELECT to_ref FROM association_evidence WHERE association_evidence_id=$1",
    )
    .bind(association.association_evidence_id.0)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(evidence_endpoint, tags[1].tag_id.0.to_string());
    let projection = rt
        .store
        .topology_projection_input(subject, true)
        .await
        .unwrap();
    assert!(
        projection
            .nodes
            .contains(&CognitiveRef::Tag(tags[0].tag_id))
    );
    assert!(
        !projection
            .nodes
            .contains(&CognitiveRef::Tag(tags[1].tag_id))
    );
    assert!(projection.edges.iter().any(|edge| edge.from
        == CognitiveRef::MemoryRevision(revisions[0])
        && edge.to == CognitiveRef::Tag(tags[0].tag_id)));
    let neighborhood = owner
        .association_neighborhood(subject, CognitiveRef::Tag(tags[1].tag_id), 64, 1)
        .await
        .unwrap();
    assert!(
        neighborhood
            .nodes
            .contains(&CognitiveRef::Tag(tags[0].tag_id))
    );
    assert_eq!(
        neighborhood.associations[0].to,
        CognitiveRef::Tag(tags[0].tag_id)
    );
    let bound = rt
        .cognition
        .bind_query(CognitiveQuery {
            subject,
            projection: Default::default(),
            temporal_frame: Default::default(),

            work_context: None,
            session: None,
            situation: Default::default(),
            expression: CognitiveQueryExpr {
                cues: vec![
                    Cue::Text(TextCue {
                        text: "release approval".into(),
                    }),
                    Cue::Tag(TagCue {
                        tag: tags[1].tag_id,
                    }),
                ],
                ..Default::default()
            },
            exploration: Default::default(),
            resources: Default::default(),
            result_need: Default::default(),
            effort: Default::default(),
            capabilities: Default::default(),
            diagnostics: Default::default(),
        })
        .await
        .unwrap();
    assert!(
        matches!(&bound.source_query.expression.cues[1],Cue::Tag(tag) if tag.tag==tags[0].tag_id)
    );
    assert!(bound.representation.text.contains("Release approval"));
    // A subsequent merge flattens interpretation; old immutable facts stay intact.
    owner
        .merge_tags(
            subject,
            MergeTagsInput {
                operation_id: OperationId::new(),
                survivor: expectation(&tags[2]),
                retired: vec![expectation(&tags[0])],
                basis: vec![basis[0].clone()],
            },
        )
        .await
        .unwrap();
    assert_eq!(
        rt.store
            .canonical_tag_id(subject, tags[1].tag_id)
            .await
            .unwrap(),
        tags[2].tag_id
    );
    let split = SplitTagInput {
        producer: None,
        operation_id: OperationId::new(),
        parent: expectation(&tags[2]),
        children: vec![content("Approval policy"), content("Rollout procedure")],
        basis: vec![basis[0].clone()],
    };
    let children = owner.split_tag(subject, split.clone()).await.unwrap();
    let replay = owner.split_tag(subject, split.clone()).await.unwrap();
    assert_eq!(
        children.iter().map(|t| t.tag_id).collect::<Vec<_>>(),
        replay.iter().map(|t| t.tag_id).collect::<Vec<_>>()
    );
    assert_eq!(
        rt.store
            .canonical_tag_id(subject, tags[2].tag_id)
            .await
            .unwrap(),
        tags[2].tag_id
    );
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE subject_id=$1")
        .bind(subject.0)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    let mut invalid = split;
    invalid.operation_id = OperationId::new();
    invalid.basis = vec![basis[0].clone(), basis[0].clone()];
    assert!(owner.split_tag(subject, invalid).await.is_err());
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM tags WHERE subject_id=$1")
        .bind(subject.0)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(before, after);
    let lineage: i64 = sqlx::query_scalar("SELECT count(*) FROM tag_lineage WHERE subject_id=$1")
        .bind(subject.0)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(lineage, 4);
}

#[tokio::test]
async fn lexical_allocation_retries_unique_conflicts_and_rolls_back_exhaustion() {
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
    let reference = |value| CognitiveRef::Entity(EntityRef::new(value).unwrap());
    let resident = rt
        .store
        .bind_identity(
            subject,
            reference("entity:proquint:resident"),
            "Resident identity".into(),
            vec!["Resident alias".into()],
        )
        .await
        .unwrap();
    // Exercise the real INSERT/ON CONFLICT path without changing production randomness.
    // Sequences survive rollback, so exhaustion can be checked after the transaction fails.
    sqlx::raw_sql(
        r"CREATE SEQUENCE proquint_collision_attempts;
        CREATE FUNCTION force_proquint_collision() RETURNS trigger LANGUAGE plpgsql AS $$
        DECLARE attempt bigint;
        BEGIN
            IF NEW.object_kind='entity' AND NEW.canonical_ref IN
                ('entity:proquint:retry','entity:proquint:exhausted') THEN
                attempt := nextval('proquint_collision_attempts');
                IF (NEW.canonical_ref='entity:proquint:retry' AND attempt<=2)
                    OR NEW.canonical_ref='entity:proquint:exhausted' THEN
                    SELECT lexical_ref INTO NEW.lexical_ref FROM lexical_bindings
                    WHERE object_kind='entity' AND canonical_ref='entity:proquint:resident';
                END IF;
            END IF;
            RETURN NEW;
        END $$;
        CREATE TRIGGER proquint_collision BEFORE INSERT ON lexical_bindings
            FOR EACH ROW EXECUTE FUNCTION force_proquint_collision();",
    )
    .execute(rt.store.pool())
    .await
    .unwrap();
    let retried = rt
        .store
        .bind_identity(
            subject,
            reference("entity:proquint:retry"),
            "Retried identity".into(),
            vec!["Retried alias".into()],
        )
        .await
        .unwrap();
    assert_ne!(retried.lexical_ref, resident.lexical_ref);
    assert_eq!(
        nous_persistence::validate_lexical(&retried.lexical_ref).unwrap(),
        "ent"
    );
    let replay = rt
        .store
        .ensure_identity_address(subject, reference("entity:proquint:retry"), String::new())
        .await
        .unwrap();
    assert_eq!(replay.lexical_ref, retried.lexical_ref);
    assert_eq!(replay.display_name, retried.display_name);
    assert_eq!(replay.aliases, retried.aliases);
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM proquint_collision_attempts")
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(attempts, 3);
    let error = rt
        .store
        .bind_identity(
            subject,
            reference("entity:proquint:exhausted"),
            "Exhausted identity".into(),
            vec!["Exhausted alias".into()],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Infrastructure(message) if message == "LexicalRef collision retry bound exceeded")
    );
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM proquint_collision_attempts")
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(attempts, 35);
    let partial: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM lexical_bindings WHERE canonical_ref='entity:proquint:exhausted'",
    )
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(partial, 0);
    let (status, unchanged) = rt
        .store
        .resolve_identity(subject, "entity", &resident.lexical_ref, true)
        .await
        .unwrap();
    assert_eq!(status, "BOUND");
    assert_eq!(unchanged.len(), 1);
    assert_eq!(unchanged[0].canonical, resident.canonical);
    assert_eq!(unchanged[0].display_name, resident.display_name);
    assert_eq!(unchanged[0].aliases, resident.aliases);
    let (status, _) = rt
        .store
        .resolve_identity(subject, "entity", "Exhausted alias", false)
        .await
        .unwrap();
    assert_eq!(status, "UNKNOWN_REFERENCE");
}

#[tokio::test]
async fn address_batch_reads_preserve_directory_and_enforce_visibility() {
    let (root, url, _postgres) = database().await;
    let rt = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&rt).await;
    let reference = |value| CognitiveRef::Entity(EntityRef::new(value).unwrap());
    let resident = rt
        .store
        .bind_identity(
            subject,
            reference("entity:proquint:resident"),
            "Resident identity".into(),
            vec!["Resident alias".into()],
        )
        .await
        .unwrap();
    let before: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM lexical_bindings),(SELECT count(*) FROM lexical_visibility),(SELECT count(*) FROM authority_object_states)").fetch_one(rt.store.pool()).await.unwrap();
    let targets = vec![
        nous_persistence::IdentityAddressTarget {
            subject,
            reference: resident.canonical.clone(),
        },
        nous_persistence::IdentityAddressTarget {
            subject,
            reference: reference("entity:proquint:missing"),
        },
        nous_persistence::IdentityAddressTarget {
            subject: nous_core::SubjectId(uuid::Uuid::new_v4()),
            reference: resident.canonical.clone(),
        },
    ];
    let addresses = rt.store.identity_addresses(&targets).await.unwrap();
    assert_eq!(addresses.len(), 3);
    assert_eq!(
        addresses[0].lexical_ref.as_ref(),
        Some(&resident.lexical_ref)
    );
    assert_eq!(addresses[0].status, "BOUND");
    assert_eq!(addresses[1].lexical_ref, None);
    assert_eq!(addresses[1].status, "UNKNOWN_REFERENCE");
    assert_eq!(addresses[2].lexical_ref, None);
    assert_eq!(addresses[2].status, "UNKNOWN_REFERENCE");
    let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM lexical_bindings),(SELECT count(*) FROM lexical_visibility),(SELECT count(*) FROM authority_object_states)").fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(after, before);
    sqlx::query("UPDATE lexical_bindings SET tombstoned_at=now() WHERE lexical_ref=$1")
        .bind(&resident.lexical_ref)
        .execute(rt.store.pool())
        .await
        .unwrap();
    let tombstone = rt.store.identity_addresses(&targets[..1]).await.unwrap();
    assert_eq!(tombstone[0].lexical_ref, None);
    assert_eq!(tombstone[0].status, "REFERENCE_TOMBSTONED");
}
