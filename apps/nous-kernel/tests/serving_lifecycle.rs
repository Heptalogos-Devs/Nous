// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{CognitiveRef, OperationId, ServingNeed};
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime_with_serving};

#[tokio::test]
async fn query_slot_releases_when_stream_permit_is_dropped() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let execution = runtime.execute_query(query(subject), None).await.unwrap();
    let slot = test_support::query_slot(&runtime.cognition, &execution.bound);

    assert!(runtime.cognition.acquire_query_slot(1).is_err());
    drop(slot);
    drop(execution);
    let permit = runtime
        .cognition
        .acquire_query_slot(1)
        .expect("dropping the stream-owned permit releases its slot");
    drop(permit);
}

#[tokio::test]
async fn cache_loss_rebuilds_at_the_same_watermark_and_retired_readers_are_protected() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let source = observation(
        &runtime,
        subject,
        "Recall relevant cognition from this source",
    )
    .await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Recall relevant cognition from the first source",
        ))
        .await
        .unwrap();
    let first = CognitiveRef::MemoryRevision(memory.revision.header.memory_revision_id());
    let request = query(subject);
    assert!(
        runtime
            .query(request.clone())
            .await
            .unwrap()
            .results
            .iter()
            .any(|hit| hit.reference == first)
    );
    let old = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "lexical")
        .unwrap_or_else(|| panic!("Expected a lexical asset"));
    let held = runtime
        .execute_query(request.clone(), Some(5))
        .await
        .unwrap();
    let next = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Recall relevant cognition from a second source",
        ))
        .await
        .unwrap();
    runtime
        .serving
        .prepare(
            subject,
            ServingNeed {
                lexical: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let busy = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(busy.readers_active);
    assert!(!busy.reclaimed.contains(&old.generation_id));
    assert!(std::path::Path::new(&old.artifact_location).exists());
    drop(held);
    let reclaimed = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(reclaimed.reclaimed.contains(&old.generation_id));
    assert!(!std::path::Path::new(&old.artifact_location).exists());
    let watermark = runtime.store.authority_seq(subject).await.unwrap();
    drop(runtime);
    let reopened = reopen_without_serving_cache(&url, &root).await;
    let rebuilt = reopened.query(request).await.unwrap();
    assert!(rebuilt.results.iter().any(|hit| hit.reference == first));
    assert!(rebuilt.results.iter().any(|hit| hit.reference
        == CognitiveRef::MemoryRevision(next.revision.header.memory_revision_id())));
    assert_eq!(
        reopened.store.authority_seq(subject).await.unwrap(),
        watermark
    );
    assert!(
        reopened
            .store
            .serving_current(subject)
            .await
            .unwrap()
            .iter()
            .all(|record| std::path::Path::new(&record.artifact_location).exists())
    );
}

#[tokio::test]
async fn purge_retry_resumes_after_serving_artifact_deletion_fails() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "artifact-delete failure source").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Remove this memory only after Serving bytes are gone",
        ))
        .await
        .unwrap();
    runtime
        .serving
        .prepare(
            subject,
            ServingNeed {
                lexical: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let artifact = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "lexical")
        .expect("lexical generation was prepared");
    let operation = OperationId::new();
    runtime
        .require_memory()
        .unwrap()
        .begin_purge_cognition(
            subject,
            CognitiveRef::Memory(memory.header.memory_id()),
            operation,
            memory.header.object_epoch,
        )
        .await
        .unwrap();
    let purge_epoch = runtime.store.serving_purge_epoch(subject).await.unwrap();

    let artifact_path = std::path::Path::new(&artifact.artifact_location);
    std::fs::remove_dir_all(artifact_path).unwrap();
    std::fs::write(artifact_path, b"simulate artifact deletion failure").unwrap();
    assert!(
        runtime
            .serving
            .reclaim_purged(subject, purge_epoch)
            .await
            .is_err()
    );
    let direct_completion = runtime
        .require_memory()
        .unwrap()
        .complete_purge_cognition(
            subject,
            CognitiveRef::Memory(memory.header.memory_id()),
            operation,
            memory.header.object_epoch,
        )
        .await;
    assert!(matches!(
        direct_completion,
        Err(nous_core::Error {
            owner: nous_core::ErrorOwner::Memory,
            code: nous_core::ErrorCode::OperationInProgress,
            ..
        })
    ));
    let adopted_checkpoint = runtime
        .require_memory()
        .unwrap()
        .complete_purge_cognition(
            subject,
            CognitiveRef::Memory(memory.header.memory_id()),
            OperationId::new(),
            memory.header.object_epoch,
        )
        .await;
    assert!(matches!(
        adopted_checkpoint,
        Err(nous_core::Error {
            owner: nous_core::ErrorOwner::Memory,
            code: nous_core::ErrorCode::OperationInProgress,
            ..
        })
    ));
    let state: String = sqlx::query_scalar(
        "SELECT state FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2",
    )
    .bind(subject.0)
    .bind(operation.0)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(state, "in_progress");

    std::fs::remove_file(artifact_path).unwrap();
    std::fs::create_dir_all(artifact_path).unwrap();
    std::fs::write(artifact_path.join("postings.json"), b"{}").unwrap();
    runtime
        .purge_cognition(
            subject,
            CognitiveRef::Memory(memory.header.memory_id()),
            operation,
            memory.header.object_epoch,
        )
        .await
        .unwrap();
    let state: String = sqlx::query_scalar(
        "SELECT state FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2",
    )
    .bind(subject.0)
    .bind(operation.0)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(state, "committed");
    assert!(!artifact_path.exists());
}

async fn reopen_without_serving_cache(
    url: &str,
    root: &tempfile::TempDir,
) -> nous_kernel::NousRuntime {
    std::fs::remove_dir_all(root.path().join("serving")).unwrap();
    let reopened = open_runtime_with_serving(url, root, true, false, false).await;
    assert!(
        std::fs::read_dir(root.path().join("serving"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .all(|entry| entry.file_name() == ".nous-serving.lock"),
        "opening Authority must not reconstruct request-owned Serving assets"
    );
    reopened
}
