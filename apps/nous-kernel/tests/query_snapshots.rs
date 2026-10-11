// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{Cue, EntityRef, OperationId};
use nous_persistence::WorkflowReservation;
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
async fn purge_between_query_binding_and_finalize_rejects_the_bound_result() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "shared source").await;
    let operation = OperationId::new();
    let entity = EntityRef::new("entity:cohort").unwrap();
    let mut input = form_input(
        subject,
        source.occurrence.occurrence_id,
        operation,
        "purge candidate",
    );
    input.aboutness = vec![entity.clone()];
    let candidate = runtime
        .require_memory()
        .unwrap()
        .form_memory(input)
        .await
        .unwrap();
    let mut request = query(subject);
    request.expression.cues = vec![
        Cue::Text(nous_core::TextCue {
            text: "Recall cognition for this entity".into(),
        }),
        Cue::Entity(nous_core::EntityCue { entity_ref: entity }),
    ];
    let execution = runtime.execute_query(request, Some(64)).await.unwrap();
    assert_eq!(execution.result.results.len(), 1);

    let key = operation.0.to_string();
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
                    dependencies: vec![nous_core::CognitiveRef::Memory(
                        candidate.header.memory_id(),
                    )],
                    ..Default::default()
                },
                ..Default::default()
            },
            360,
        )
        .await
        .unwrap();
    let WorkflowReservation::Acquired { lease, .. } = reservation else {
        panic!("new model workflow reservation should be acquired");
    };
    let lease = lease.token;
    runtime
        .store
        .save_model_workflow(
            &test_support::workflow_lease(subject, "memory", &key, lease),
            Some(&test_support::workflow_payload(
                serde_json::json!({"text":"private-cognitive-marker"}),
            )),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    runtime
        .purge_cognition(
            subject,
            nous_core::CognitiveRef::Memory(candidate.header.memory_id()),
            OperationId::new(),
            candidate.header.object_epoch,
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
                None,
            )
            .await
            .is_err()
    );

    let error = test_support::finish_query_execution(
        &runtime.cognition,
        execution,
        Vec::new(),
        Vec::new(),
        nous_runtime::CognitiveContributors {
            material: Some(&runtime.material),
            shared: None,
            memory: Some(runtime.require_memory().unwrap()),
        },
    )
    .await
    .expect_err("purge fence rejects the whole stale query result");
    assert_eq!(error.owner, nous_core::ErrorOwner::Runtime);
    assert_eq!(error.code, nous_core::ErrorCode::StaleContext);
    assert_eq!(
        error.context.get("reason").map(String::as_str),
        Some("purge_epoch_changed")
    );
}
