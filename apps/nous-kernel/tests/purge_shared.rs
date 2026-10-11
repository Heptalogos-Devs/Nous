// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
async fn purge_keeps_shared_observation_authority() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let source = observation(&runtime, subject, "shared source material").await;
    let first = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            nous_core::OperationId::new(),
            "first cognition",
        ))
        .await
        .expect("first memory");
    let second = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            nous_core::OperationId::new(),
            "second cognition",
        ))
        .await
        .expect("second memory");
    let artifact_id = source.artifact.as_ref().expect("artifact").artifact_id;
    runtime
        .purge_cognition(
            subject,
            first.reference.clone(),
            nous_core::OperationId::new(),
            first.object_epoch.unwrap(),
        )
        .await
        .expect("purge first cognition");
    assert!(
        runtime
            .require_memory()
            .unwrap()
            .read_cognition(subject, second.reference.clone(),)
            .await
            .is_ok()
    );
    assert!(
        runtime
            .material
            .artifact(subject, artifact_id)
            .await
            .is_ok()
    );
}
