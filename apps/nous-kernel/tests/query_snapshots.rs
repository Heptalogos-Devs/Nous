// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{Cue, EntityRef, OperationId};
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

    runtime
        .purge_cognition(
            subject,
            candidate.reference.clone(),
            OperationId::new(),
            candidate.object_epoch.unwrap(),
        )
        .await
        .unwrap();
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
