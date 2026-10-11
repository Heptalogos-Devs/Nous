// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::Utc;
use nous_core::{
    CognitiveQuery, CognitiveQueryExpr, CognitiveRef, Cue, EntityRef, EpistemicClass, OperationId,
    QueryOperation, QueryTarget, ResultNeed, TemporalExtent, TextCue, UseEventId,
};
use nous_memory::{BasisRole, EvidenceLocator, EvidenceRef, FormationMode, RevisionBasis};
use nous_runtime::{QueryPlan, UseFeedback, UseFeedbackEvent, UseKind};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime, open_runtime_with_serving};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one retained query is finalized after revise, suppress and purge, including the purged proposal fence"
)]
async fn rerank_revalidates_original_candidates_after_revise_suppress_and_purge() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "shared source").await;
    let entity = EntityRef::new("entity:cohort").unwrap();
    let mut cohort = Vec::new();
    let mut operations = Vec::new();
    for index in 0..3 {
        let operation = OperationId::new();
        operations.push(operation);
        let mut input = form_input(
            subject,
            source.occurrence.occurrence_id,
            operation,
            &format!("candidate {index}"),
        );
        input.aboutness = vec![entity.clone()];
        cohort.push(
            runtime
                .require_memory()
                .unwrap()
                .form_memory(input)
                .await
                .unwrap(),
        );
    }
    let mut request = query(subject);
    request.expression.cues = vec![
        Cue::Text(nous_core::TextCue {
            text: "Recall cognition for this entity".into(),
        }),
        Cue::Entity(nous_core::EntityCue { entity_ref: entity }),
    ];
    let execution = runtime.execute_query(request, Some(64)).await.unwrap();
    assert_eq!(execution.result.results.len(), 3);
    let expiring = nous_runtime::QueryLease::new(std::time::Duration::from_millis(200)).unwrap();
    let (_, expired_ticket) = runtime
        .cognition
        .retain_query(execution.clone(), expiring)
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(
        runtime
            .cognition
            .finalize_query(
                subject,
                expired_ticket.unwrap(),
                Vec::new(),
                Vec::new(),
                nous_runtime::CognitiveContributors {
                    material: Some(&runtime.material),
                    shared: None,
                    memory: Some(runtime.require_memory().unwrap()),
                }
            )
            .await
            .is_err(),
        "validation must retain the original opportunity deadline"
    );
    let (pool, ticket) = runtime
        .cognition
        .retain_query(execution, test_support::query_lease())
        .unwrap();
    let ticket = ticket.expect("validated query snapshot should be retained");
    let first = &cohort[0];
    runtime
        .require_memory()
        .unwrap()
        .revise_memory(nous_memory::ReviseMemoryInput {
            producer: None,
            operation_id: OperationId::new(),
            subject,
            memory_id: first.object.memory_id,
            expected_object_epoch: first.object.object_epoch,
            intent: nous_memory::RevisionIntent::Correct,
            formation_mode: FormationMode::Grounded,
            grounding_occurrence_id: Some(source.occurrence.occurrence_id),
            semantic_role: "fact".into(),
            representation_text: "replacement candidate".into(),
            title: None,
            basis: first.basis.clone(),
            aboutness: first.aboutness.clone(),
            valid_time: TemporalExtent::Unknown,

            epistemic_class: EpistemicClass::Observed,
        })
        .await
        .unwrap();
    let second = &cohort[1];
    runtime
        .require_memory()
        .unwrap()
        .suppress(
            subject,
            second.object.memory_id,
            OperationId::new(),
            second.object.object_epoch,
        )
        .await
        .unwrap();
    let third = &cohort[2];
    let key = operations[2].0.to_string();
    let reservation = runtime
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "semantic input",
            &nous_core::WorkflowSnapshot {
                content: nous_core::WorkflowPayload {
                    payload: serde_json::json!({"model":"frozen"}),
                    dependencies: vec![nous_core::CognitiveRef::Memory(third.object.memory_id)],
                    ..Default::default()
                },
                ..Default::default()
            },
            360,
        )
        .await
        .unwrap();
    let lease = reservation.lease.as_ref().unwrap().token;
    runtime
        .store
        .save_model_workflow(
            &test_support::workflow_lease(subject, "memory", &key, lease),
            Some(&test_support::workflow_payload(
                serde_json::json!({"text":"private-cognitive-marker"}),
            )),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    runtime
        .require_memory()
        .unwrap()
        .purge_memory(
            subject,
            third.object.memory_id,
            OperationId::new(),
            third.object.object_epoch,
        )
        .await
        .unwrap();
    use sqlx::Row;
    let row = sqlx::query("SELECT snapshot,proposal,outcome FROM model_workflow_operations WHERE subject_id=$1 AND owner='memory' AND operation_key=$2").bind(subject.0).bind(&key).fetch_one(runtime.store.pool()).await.unwrap();
    assert_eq!(
        row.get::<serde_json::Value, _>("snapshot"),
        serde_json::to_value(nous_core::WorkflowSnapshot::default()).unwrap()
    );
    assert!(
        row.get::<Option<serde_json::Value>, _>("proposal")
            .is_none()
    );
    assert_eq!(
        row.get::<serde_json::Value, _>("outcome"),
        serde_json::to_value(nous_core::WorkflowPayload {
            purged: true,
            ..Default::default()
        })
        .unwrap()
    );
    assert!(
        runtime
            .store
            .save_model_workflow(
                &test_support::workflow_lease(subject, "memory", &key, lease),
                Some(&test_support::workflow_payload(
                    serde_json::json!({"text":"resurrect"})
                )),
                None,
                None,
                &[],
            )
            .await
            .is_err()
    );
    let order = pool
        .results
        .into_iter()
        .map(|hit| (hit.reference, 1.0))
        .collect();
    let result = runtime
        .cognition
        .finalize_query(
            subject,
            ticket,
            order,
            Vec::new(),
            nous_runtime::CognitiveContributors {
                material: Some(&runtime.material),
                shared: None,
                memory: Some(runtime.require_memory().unwrap()),
            },
        )
        .await
        .unwrap();
    assert!(result.results.is_empty());
    assert!(
        result
            .degradation
            .iter()
            .any(|value| value.code == "authority_changed_during_rerank")
    );
    assert_eq!(
        result.diagnostics.unwrap().candidate_counts["drop_authority_changed_during_rerank"],
        3
    );
    assert!(runtime.cognition.release_query(subject, ticket).is_ok());
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one exact-read cohort checks candidate closure, immutable history, as-of heads and mutation fencing against the same unrelated resident"
)]
async fn exact_mutable_binding_is_fenced_and_explicit_history_is_readable() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let observation = observation(&runtime, subject, "fenced fact").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            observation.occurrence.occurrence_id,
            OperationId::new(),
            "old fenced fact",
        ))
        .await
        .expect("memory");
    let mut request = query(subject);
    request.expression.targets = vec![QueryTarget::Exact {
        reference: CognitiveRef::Memory(memory.object.memory_id),
    }];
    let unrelated = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            observation.occurrence.occurrence_id,
            OperationId::new(),
            "Recall relevant cognition: an unrelated resident",
        ))
        .await
        .unwrap();
    let session = runtime
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    runtime
        .cognition
        .use_feedback(UseFeedback {
            subject,
            session_id: Some(session.session_id),
            consumer_ref: "consumer:test:exact".into(),
            events: vec![UseFeedbackEvent {
                query_id: None,
                event_id: UseEventId::new(),
                reference: CognitiveRef::MemoryRevision(unrelated.revision.memory_revision_id),
                use_kind: UseKind::Referenced,
                occurred_at: Utc::now(),
                context: serde_json::json!({}),
            }],
        })
        .await
        .unwrap();
    request.session = Some(session.session_id);
    let exact = runtime.query(request.clone()).await.unwrap();
    assert_eq!(
        exact.results.len(),
        1,
        "exact read must not include residents or similarity hits"
    );
    assert_eq!(
        exact.results[0].reference,
        CognitiveRef::MemoryRevision(memory.revision.memory_revision_id)
    );
    assert_eq!(exact.status, nous_core::QueryStatus::Complete);
    assert!(
        exact.degradation.is_empty(),
        "exact read must not depend on unavailable Serving: {:?}",
        exact.degradation
    );
    assert!(exact.generation.lexical.is_none());
    let mut asof_request = request.clone();
    asof_request.temporal_frame.authority_view = nous_core::AuthorityView::AsOf(Utc::now());
    let mut head_request = request.clone();
    head_request.temporal_frame.revision_view = nous_core::RevisionView::History;
    let bound = runtime.cognition.bind_query(request).await.expect("bind");
    let plan = QueryPlan::for_bound_query(&bound);
    let revision = nous_memory::ReviseMemoryInput {
        producer: None,
        operation_id: OperationId::new(),
        subject,
        memory_id: memory.object.memory_id,
        expected_object_epoch: memory.object.object_epoch,
        intent: nous_memory::RevisionIntent::Correct,
        formation_mode: FormationMode::Grounded,
        grounding_occurrence_id: Some(observation.occurrence.occurrence_id),
        semantic_role: "fact".into(),
        representation_text: "new fenced fact".into(),
        title: None,
        basis: vec![RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: None,
            occurrence_id: observation.occurrence.occurrence_id,
            locator: EvidenceLocator::WholeOccurrence,
            basis_role: BasisRole::Direct,
        })],
        aboutness: Vec::new(),
        valid_time: Default::default(),

        epistemic_class: EpistemicClass::Observed,
    };
    let _current = runtime
        .require_memory()
        .unwrap()
        .revise_memory(revision.clone())
        .await
        .expect("revision");
    let memory_service = runtime.require_memory().unwrap();
    let result = runtime
        .cognition
        .query_with_plan(
            bound,
            nous_runtime::CognitiveContributors {
                material: Some(&runtime.material),
                shared: None,
                memory: Some(memory_service as &dyn nous_runtime::CognitiveContributor),
            },
            plan,
        )
        .await
        .expect("stale exact query");
    assert!(result.results.is_empty());
    assert_eq!(
        result
            .diagnostics
            .as_ref()
            .and_then(|value| value.candidate_counts.get("drop_stale_exact_binding"))
            .copied(),
        Some(1),
        "query diagnostics: {:?}",
        result.diagnostics
    );
    let asof = runtime.query(asof_request).await.unwrap();
    assert_eq!(asof.results.len(), 1);
    assert_eq!(
        asof.results[0].reference,
        CognitiveRef::MemoryRevision(memory.revision.memory_revision_id)
    );
    let current_head = runtime.query(head_request).await.unwrap();
    assert_eq!(
        current_head.results.len(),
        1,
        "history permits old exact revisions but does not expand an exact object into revision enumeration"
    );
    assert_eq!(
        current_head.results[0].reference,
        CognitiveRef::MemoryRevision(_current.revision.memory_revision_id)
    );
    let historical = runtime
        .query(CognitiveQuery {
            projection: Default::default(),
            temporal_frame: Default::default(),

            work_context: None,
            subject,
            session: None,
            situation: Default::default(),
            expression: CognitiveQueryExpr {
                operation: QueryOperation::Atom,
                preferences: Vec::new(),
                children: Vec::new(),
                targets: vec![QueryTarget::Exact {
                    reference: CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
                }],
                cues: vec![Cue::Text(nous_core::TextCue {
                    text: "Recall relevant cognition".into(),
                })],
                constraints: Default::default(),
            },
            exploration: Default::default(),
            resources: Default::default(),
            result_need: ResultNeed {
                limit: 1,
                ..Default::default()
            },
            effort: Default::default(),
            capabilities: Default::default(),
            diagnostics: Default::default(),
        })
        .await
        .expect("historical query");
    assert_eq!(
        historical.results[0].reference,
        CognitiveRef::MemoryRevision(memory.revision.memory_revision_id)
    );
}

#[tokio::test]
async fn subject_profiles_change_new_queries_and_preserve_inflight_preparation() {
    use nous_runtime::{COGNITIVE_PROFILE, CognitiveProfile};
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, true).await;
    let mut subjects = Vec::new();
    for _ in 0..2 {
        subjects.push(
            runtime
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
                .subject_id,
        );
    }
    runtime
        .configuration
        .set_subject_override(
            OperationId::new(),
            subjects[0],
            COGNITIVE_PROFILE.path(),
            serde_json::json!("baseline-rrf"),
            None,
        )
        .await
        .unwrap();
    runtime
        .configuration
        .set_subject_override(
            OperationId::new(),
            subjects[1],
            COGNITIVE_PROFILE.path(),
            serde_json::json!("vcp-rivermemo-v3.1-adapter-v1"),
            None,
        )
        .await
        .unwrap();
    let prepared_query = |subject| {
        let mut q = query(subject);
        q.expression.cues.push(Cue::Text(TextCue {
            text: "A closed semantic intent".into(),
        }));
        q.exploration = nous_core::ExplorationIntent::BoundedAssociative;
        q
    };
    let frozen = runtime
        .cognition
        .bind_query(prepared_query(subjects[0]))
        .await
        .unwrap();
    let second = runtime
        .cognition
        .bind_query(prepared_query(subjects[1]))
        .await
        .unwrap();
    assert_eq!(
        frozen.config_snapshot.get(COGNITIVE_PROFILE).unwrap(),
        CognitiveProfile::BaselineRrf
    );
    assert_eq!(
        second.config_snapshot.get(COGNITIVE_PROFILE).unwrap(),
        CognitiveProfile::VcpRiverMemo
    );
    runtime
        .configuration
        .set_subject_override(
            OperationId::new(),
            subjects[0],
            COGNITIVE_PROFILE.path(),
            serde_json::json!("nous-node-potential-v1"),
            None,
        )
        .await
        .unwrap();
    let changed = runtime
        .cognition
        .bind_query(prepared_query(subjects[0]))
        .await
        .unwrap();
    let unchanged = runtime
        .cognition
        .bind_query(prepared_query(subjects[1]))
        .await
        .unwrap();
    assert_eq!(
        changed.config_snapshot.get(COGNITIVE_PROFILE).unwrap(),
        CognitiveProfile::NousNodePotential
    );
    assert_eq!(
        unchanged.config_snapshot.get(COGNITIVE_PROFILE).unwrap(),
        CognitiveProfile::VcpRiverMemo
    );
    assert_eq!(
        frozen.config_snapshot.get(COGNITIVE_PROFILE).unwrap(),
        CognitiveProfile::BaselineRrf
    );
    assert_eq!(frozen.representation.text, changed.representation.text);
    assert_ne!(
        frozen
            .config_snapshot
            .digest_for(&[COGNITIVE_PROFILE.path()])
            .unwrap(),
        changed
            .config_snapshot
            .digest_for(&[COGNITIVE_PROFILE.path()])
            .unwrap()
    );
    assert_ne!(frozen.enabled_lanes, changed.enabled_lanes);
}
