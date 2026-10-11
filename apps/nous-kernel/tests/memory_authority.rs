// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::Utc;
use nous_core::{CognitiveRef, OperationId, QueryTarget, UseEventId};
use nous_memory::{
    BasisRole, CognitionAction, CreateSchemaInput, EvidenceLocator, EvidenceRef, RevisionBasis,
    SchemaEvidenceLinkInput, SchemaEvidenceRole, SchemaFormationKind, SchemaScope,
};
use nous_runtime::{UseFeedback, UseFeedbackEvent, UseKind};
use test_support::query::{query, subject};
use test_support::{database, observation, open_runtime};

#[tokio::test]
async fn schema_lifecycle_fences_queries_and_purge_cannot_replay_content() {
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
    let schema_id = schema.header.schema_id();
    let revision_id = schema.revision.header.schema_revision_id();
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
    let target = CognitiveRef::CognitiveSchema(schema_id);
    let suppressed = owner
        .mutate_cognition(
            subject,
            target.clone(),
            suppress_operation,
            1,
            CognitionAction::Suppress,
        )
        .await
        .unwrap();
    assert_eq!(suppressed.header().object_epoch, 2);
    assert_eq!(
        suppressed.revision_header().revision_ref,
        CognitiveRef::CognitiveSchemaRevision(revision_id)
    );
    let (result_kind, result_ref, result_revision, result_epoch): (
        Option<String>,
        Option<String>,
        Option<uuid::Uuid>,
        Option<i64>,
    ) = sqlx::query_as("SELECT result_kind,result_ref,result_revision,result_epoch FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(suppress_operation.0)
        .fetch_one(runtime.store.pool())
        .await
        .unwrap();
    assert_eq!(result_kind.as_deref(), Some("cognitive_schema"));
    assert_eq!(result_ref, Some(schema_id.0.to_string()));
    assert_eq!(result_revision, Some(revision_id.0));
    assert_eq!(result_epoch, Some(2));
    assert!(runtime.query(exact()).await.unwrap().results.is_empty());
    assert!(matches!(
        owner
            .mutate_cognition(
                subject,
                target.clone(),
                OperationId::new(),
                1,
                CognitionAction::Restore
            )
            .await,
        Err(nous_core::Error {
            code: nous_core::ErrorCode::StaleRevision,
            ..
        })
    ));
    owner
        .mutate_cognition(
            subject,
            target.clone(),
            OperationId::new(),
            2,
            CognitionAction::Restore,
        )
        .await
        .unwrap();
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    owner
        .mutate_cognition(
            subject,
            target.clone(),
            OperationId::new(),
            3,
            CognitionAction::Withdraw,
        )
        .await
        .unwrap();
    assert!(runtime.query(exact()).await.unwrap().results.is_empty());
    owner
        .mutate_cognition(
            subject,
            target.clone(),
            OperationId::new(),
            4,
            CognitionAction::Reaccept,
        )
        .await
        .unwrap();
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    let revised_schema = owner
        .revise_schema(nous_memory::ReviseSchemaInput {
            operation_id: OperationId::new(),
            subject,
            schema_id,
            expected_object_epoch: 5,
            intent: nous_memory::RevisionIntent::Rephrase,
            copy_link_ids: Vec::new(),
            content: nous_memory::SchemaContent {
                producer: None,
                title: schema.revision.title.clone(),
                structural_claim: "Continue an open task from its selected durable context".into(),
                applicability_scope: schema.revision.applicability_scope.clone(),
                boundary_definition: schema.revision.boundary_definition.clone(),
                formation_kind: schema.revision.formation_kind,
                evidence_links: schema
                    .evidence_links
                    .iter()
                    .map(|link| SchemaEvidenceLinkInput {
                        role: link.role,
                        basis: link.basis.clone(),
                    })
                    .collect(),
            },
        })
        .await
        .unwrap();
    assert_eq!(revised_schema.header.object_epoch, 6);
    let replay = owner
        .mutate_cognition(
            subject,
            target.clone(),
            suppress_operation,
            1,
            CognitionAction::Suppress,
        )
        .await
        .unwrap();
    assert_eq!(replay.header().object_epoch, 6);
    assert_eq!(
        replay.header().lifecycle.suppression,
        nous_core::SuppressionState::Normal
    );
    assert_eq!(
        replay.revision_header().revision_ref,
        CognitiveRef::CognitiveSchemaRevision(revision_id)
    );
    assert_eq!(
        replay.header().current_revision_ref,
        CognitiveRef::CognitiveSchemaRevision(revised_schema.revision.header.schema_revision_id())
    );
    assert_eq!(runtime.query(exact()).await.unwrap().results.len(), 1);
    let purge_operation = OperationId::new();
    runtime
        .purge_cognition(
            subject,
            CognitiveRef::CognitiveSchema(schema_id),
            purge_operation,
            6,
        )
        .await
        .unwrap();
    let duplicate = runtime.cognition.use_feedback(use_input).await.unwrap();
    assert_eq!((duplicate.0, duplicate.1), (0, 1));
    runtime
        .purge_cognition(
            subject,
            CognitiveRef::CognitiveSchema(schema_id),
            purge_operation,
            6,
        )
        .await
        .unwrap();
    assert!(matches!(
        owner
            .read_cognition(subject, CognitiveRef::CognitiveSchemaRevision(revision_id),)
            .await,
        Err(nous_core::Error {
            code: nous_core::ErrorCode::NotFound,
            ..
        })
    ));
    assert!(matches!(
        owner
            .mutate_cognition(
                subject,
                target,
                suppress_operation,
                1,
                CognitionAction::Suppress
            )
            .await,
        Err(nous_core::Error {
            code: nous_core::ErrorCode::NotFound,
            ..
        })
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
    assert!(matches!(
        same_root,
        Err(nous_core::Error {
            code: nous_core::ErrorCode::InvalidRequest,
            ..
        })
    ));
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
