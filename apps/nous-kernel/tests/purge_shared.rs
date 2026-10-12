// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use nous_core::{CognitiveRef, OperationId, UseEventId};
use nous_memory::{ReviseMemoryInput, RevisionIntent};
use nous_runtime::{UseFeedback, UseFeedbackEvent, UseKind};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
async fn purge_keeps_shared_observation_authority() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
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
        .expect("subject")
        .subject_id;
    let memory = runtime.require_memory().unwrap();
    let source = observation(&runtime, subject, "shared source material").await;
    let first = memory
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            nous_core::OperationId::new(),
            "first cognition",
        ))
        .await
        .expect("first memory");
    let revised = memory
        .revise_memory(ReviseMemoryInput {
            content: form_input(
                subject,
                source.occurrence.occurrence_id,
                OperationId::new(),
                "first cognition revised",
            ),
            memory_id: first.object.memory_id,
            expected_object_epoch: first.object.object_epoch,
            intent: RevisionIntent::Rephrase,
        })
        .await
        .expect("second revision");
    let second = memory
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            nous_core::OperationId::new(),
            "second cognition",
        ))
        .await
        .expect("second memory");
    let artifact_id = source.artifact.as_ref().expect("artifact").artifact_id;
    let consumer_ref = "consumer:test:memory-purge";
    let use_events = [
        (UseEventId::new(), first.revision.memory_revision_id),
        (UseEventId::new(), revised.revision.memory_revision_id),
    ];
    let feedback = UseFeedback {
        subject,
        session_id: None,
        consumer_ref: consumer_ref.into(),
        events: use_events
            .iter()
            .map(|(event_id, revision)| UseFeedbackEvent {
                query_id: None,
                event_id: *event_id,
                reference: CognitiveRef::MemoryRevision(*revision),
                use_kind: UseKind::Referenced,
                occurred_at: chrono::Utc::now(),
                context: serde_json::json!({}),
            })
            .collect(),
    };
    assert_eq!(
        runtime
            .cognition
            .use_feedback(feedback.clone())
            .await
            .unwrap()
            .0,
        2
    );
    let purge_operation = OperationId::new();
    for _ in 0..2 {
        memory
            .purge_memory(
                subject,
                first.object.memory_id,
                purge_operation,
                revised.object.object_epoch,
            )
            .await
            .expect("purge and same-operation replay");
    }
    let (active_uses, purged_uses): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM cognitive_use_events WHERE subject_id=$1 AND consumer_ref=$2), (SELECT count(*) FROM purged_use_receipts WHERE subject_id=$1 AND consumer_ref=$2)",
    )
    .bind(subject.0)
    .bind(consumer_ref)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!((active_uses, purged_uses), (0, 2));
    assert_eq!(runtime.cognition.use_feedback(feedback).await.unwrap().1, 2);
    assert!(
        memory
            .memory(subject, second.object.memory_id, None)
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
