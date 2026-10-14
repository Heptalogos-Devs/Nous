// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{CognitiveRef, OperationId, ServingNeed};
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime_with_serving};

#[tokio::test]
async fn serving_reuse_checks_effective_policy_identity() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, true).await;
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
    // A cache with different effective builder policy cannot authorize reuse.
    sqlx::query("UPDATE serving_generations SET config_digest='outdated' WHERE generation_id=$1")
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
    assert_eq!(
        runtime
            .serving
            .family_status(subject)
            .await
            .unwrap()
            .into_iter()
            .find(|family| family.name == "lexical")
            .unwrap()
            .config_digest
            .as_deref(),
        Some(replacement.config_digest.as_str())
    );
    // Retired real assets retain a research pin until explicitly released.
    runtime
        .serving
        .pin_research_generation(original.generation_id, true)
        .await
        .unwrap();
    let report = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(!report.reclaimed.contains(&original.generation_id));
    assert!(
        runtime
            .serving
            .artifact_directory(original.generation_id)
            .exists()
    );
    runtime
        .serving
        .pin_research_generation(original.generation_id, false)
        .await
        .unwrap();
    let report = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(report.reclaimed.contains(&original.generation_id));

    assert_native_projection_boundaries(&runtime, subject).await;
}

async fn assert_native_projection_boundaries(
    runtime: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
) {
    // Native structure follows its own inputs, rather than every Subject write.
    let topology_need = ServingNeed {
        topology: true,
        ..Default::default()
    };
    runtime
        .serving
        .prepare(subject, topology_need)
        .await
        .unwrap();
    let topology = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "topology" && record.space.is_empty())
        .unwrap();
    let source = observation(runtime, subject, "Text alone changes no native graph node").await;
    assert!(runtime.store.authority_seq(subject).await.unwrap() > topology.authority_watermark);
    let families = runtime.serving.family_status(subject).await.unwrap();
    let native = families
        .iter()
        .find(|family| family.name == "topology" && family.space_signature.is_empty())
        .unwrap();
    assert_eq!(native.state, nous_runtime::LaneStatus::Ready);
    assert_eq!(native.desired_authority_seq, topology.authority_watermark);
    assert!(
        runtime
            .store
            .promote_generation(topology.generation_id)
            .await
            .unwrap()
    );
    let reused = runtime
        .serving
        .prepare(subject, topology_need)
        .await
        .unwrap();
    assert!(!reused.rebuilt.contains(&"topology".to_owned()));
    assert_eq!(reused.generations["topology"], topology.generation_id);

    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Recall relevant cognition with a real native graph node",
        ))
        .await
        .unwrap();
    let families = runtime.serving.family_status(subject).await.unwrap();
    let native = families
        .iter()
        .find(|family| family.name == "topology" && family.space_signature.is_empty())
        .unwrap();
    assert_eq!(native.state, nous_runtime::LaneStatus::Stale);
    assert!(
        !runtime
            .store
            .promote_generation(topology.generation_id)
            .await
            .unwrap()
    );
    let updated = runtime
        .serving
        .prepare(subject, topology_need)
        .await
        .unwrap();
    assert!(updated.rebuilt.contains(&"topology".to_owned()));
    assert_ne!(updated.generations["topology"], topology.generation_id);
    let reference = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    assert!(
        runtime
            .store
            .topology_projection_input(subject, true, None)
            .await
            .unwrap()
            .nodes
            .contains(&reference)
    );
    assert!(
        runtime
            .query(query(subject))
            .await
            .unwrap()
            .results
            .iter()
            .any(|hit| hit.reference == reference)
    );
    let families = runtime.serving.family_status(subject).await.unwrap();
    assert_eq!(
        families
            .iter()
            .find(|family| family.name == "topology" && family.space_signature.is_empty())
            .unwrap()
            .state,
        nous_runtime::LaneStatus::Ready
    );
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
    let (_, ticket) = runtime
        .cognition
        .retain_query(held, test_support::query_lease())
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
    assert!(
        runtime
            .serving
            .artifact_directory(old.generation_id)
            .exists()
    );
    runtime.cognition.release_query(subject, ticket).unwrap();
    let reclaimed = runtime
        .serving
        .reclaim_retired(subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(reclaimed.reclaimed.contains(&old.generation_id));
    assert!(
        !runtime
            .serving
            .artifact_directory(old.generation_id)
            .exists()
    );
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
            .next()
            .is_none(),
        "opening Authority must not reconstruct request-owned Serving assets"
    );
    reopened
}
