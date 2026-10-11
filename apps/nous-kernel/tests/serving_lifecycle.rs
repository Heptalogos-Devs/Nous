// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{CognitiveRef, OperationId, ServingNeed};
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime_with_serving};

#[tokio::test]
async fn serving_reuse_reads_the_typed_implementation_identity() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    observation(&runtime, subject, "Retain this original material").await;
    let need = ServingNeed {
        lexical: true,
        ..Default::default()
    };
    runtime.serving.prepare(subject, need).await.unwrap();
    let original = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "lexical")
        .unwrap();
    // The record is the owner of identity. A stale metadata copy must not authorize reuse.
    sqlx::query(
        "UPDATE serving_generations SET implementation_revision='outdated' WHERE generation_id=$1",
    )
    .bind(original.generation_id.0)
    .execute(runtime.store.pool())
    .await
    .unwrap();
    let status = runtime.serving.prepare(subject, need).await.unwrap();
    assert!(status.rebuilt.contains(&"lexical".to_owned()));
    let replacement = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "lexical")
        .unwrap();
    assert_ne!(replacement.generation_id, original.generation_id);
    assert_eq!(
        replacement.authority_watermark,
        original.authority_watermark
    );
    assert_eq!(replacement.implementation_revision, "lexical-v1");
    for key in ["implementation", "implementation_revision", "config_digest"] {
        assert!(replacement.metadata.get(key).is_none());
    }
    // An existing superseded cache must leave current without losing a research pin.
    let mut obsolete = replacement.clone();
    obsolete.generation_id = nous_core::ServingGenerationId::new();
    obsolete.family = "exact".into();
    obsolete.artifact_location = root
        .path()
        .join("serving")
        .join(obsolete.generation_id.0.to_string())
        .to_string_lossy()
        .into_owned();
    obsolete.metadata = serde_json::json!({"research_pinned":true});
    std::fs::create_dir_all(&obsolete.artifact_location).unwrap();
    std::fs::write(
        std::path::Path::new(&obsolete.artifact_location).join("postings.json"),
        b"{}",
    )
    .unwrap();
    let obsolete = runtime.store.publish_generation(obsolete).await.unwrap();
    let report = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(!report.reclaimed.contains(&obsolete.generation_id));
    assert!(std::path::Path::new(&obsolete.artifact_location).exists());
    assert!(
        runtime
            .store
            .serving_current(subject)
            .await
            .unwrap()
            .iter()
            .all(|record| record.family != "exact")
    );
    runtime
        .serving
        .pin_research_generation(obsolete.generation_id, false)
        .await
        .unwrap();
    let report = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(report.reclaimed.contains(&obsolete.generation_id));
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
    let first = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
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
    let (_, handle) =
        test_support::retain_query_run(&runtime.cognition, held, test_support::query_lease())
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
    runtime.cognition.cancel_query_run(subject, handle).unwrap();
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
    assert!(
        rebuilt
            .results
            .iter()
            .any(|hit| hit.reference
                == CognitiveRef::MemoryRevision(next.revision.memory_revision_id))
    );
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
async fn purge_retry_resumes_after_the_old_query_reader_releases() {
    use nous_core::{Error, ErrorCode, ErrorOwner};

    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "reader-held source").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Recall relevant cognition that awaits a purge",
        ))
        .await
        .unwrap();
    let query = query(subject);
    runtime.query(query.clone()).await.unwrap();
    let previous = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "lexical")
        .expect("query prepared a lexical generation");
    let (held, handle) = test_support::retain_query_run(
        &runtime.cognition,
        runtime.execute_query(query, Some(5)).await.unwrap(),
        test_support::query_lease(),
    )
    .unwrap();
    assert!(held.results.iter().any(|hit| {
        hit.reference == CognitiveRef::MemoryRevision(memory.revision.memory_revision_id)
    }));

    let operation = OperationId::new();
    let first_attempt = runtime
        .purge_memory(
            subject,
            memory.object.memory_id,
            operation,
            memory.object.object_epoch,
        )
        .await;
    assert!(matches!(
        first_attempt,
        Err(Error {
            owner: ErrorOwner::Persistence,
            code: ErrorCode::OperationInProgress,
            ..
        })
    ));
    assert!(std::path::Path::new(&previous.artifact_location).exists());

    runtime.cognition.cancel_query_run(subject, handle).unwrap();
    runtime
        .purge_memory(
            subject,
            memory.object.memory_id,
            operation,
            memory.object.object_epoch,
        )
        .await
        .unwrap();
    assert!(!std::path::Path::new(&previous.artifact_location).exists());
    assert!(
        runtime
            .require_memory()
            .unwrap()
            .memory(subject, memory.object.memory_id, None)
            .await
            .is_err()
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
        .begin_purge_memory(
            subject,
            memory.object.memory_id,
            operation,
            memory.object.object_epoch,
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
        .complete_purge_memory(
            subject,
            memory.object.memory_id,
            operation,
            memory.object.object_epoch,
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
        .complete_purge_memory(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            memory.object.object_epoch,
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
        .purge_memory(
            subject,
            memory.object.memory_id,
            operation,
            memory.object.object_epoch,
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
