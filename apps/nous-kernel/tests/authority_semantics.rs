// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_persistence::{
    DenseInvalidation, MutationEnvelope, MutationStart, OwnerLock, ProjectionInvalidation,
};

use nous_core::{OperationId, SubjectId};

mod test_support;

#[tokio::test]
async fn mutation_envelope_merges_families_and_replays_checkpoints() {
    let (root, url, _postgres) = test_support::database().await;
    let rt = test_support::open_runtime(&url, &root).await;
    let subject = envelope_subject(&rt).await;
    let digest = "a".repeat(64);
    let operation = OperationId::new();
    let owner = Some(OwnerLock("memory-authority"));
    let initial = rt.store.authority_seq(subject).await.unwrap();
    let MutationStart::Active(mut mutation) = MutationEnvelope::begin(
        &rt.store,
        subject,
        operation,
        "test_mutation",
        &digest,
        owner,
    )
    .await
    .unwrap() else {
        panic!("new mutation");
    };
    mutation
        .invalidate(ProjectionInvalidation::text())
        .await
        .unwrap();
    drop(mutation);
    // Abandoned owner work rolls back both its sequence and receipt.
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), initial);
    let MutationStart::Active(mut mutation) = MutationEnvelope::begin(
        &rt.store,
        subject,
        operation,
        "test_mutation",
        &digest,
        owner,
    )
    .await
    .unwrap() else {
        panic!("rollback must permit retry");
    };
    let sequence = mutation
        .invalidate(ProjectionInvalidation {
            dense: DenseInvalidation::Spaces(vec!["space-a".into()]),
            ..ProjectionInvalidation::default()
        })
        .await
        .unwrap();
    assert_eq!(sequence, initial + 1);
    assert_eq!(
        mutation
            .invalidate(ProjectionInvalidation::topology())
            .await
            .unwrap(),
        sequence
    );
    mutation.checkpoint().await.unwrap();
    let MutationStart::Replay(receipt) = MutationEnvelope::begin(
        &rt.store,
        subject,
        operation,
        "test_mutation",
        &digest,
        owner,
    )
    .await
    .unwrap() else {
        panic!("checkpoint must replay");
    };
    assert_eq!(receipt.state, "in_progress");
    assert!(matches!(
        MutationEnvelope::resume(
            &rt.store,
            subject,
            operation,
            "test_mutation",
            "different",
            owner
        )
        .await,
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::OperationIdConflict
    ));
    let MutationStart::Active(mutation) = MutationEnvelope::resume(
        &rt.store,
        subject,
        operation,
        "test_mutation",
        &digest,
        owner,
    )
    .await
    .unwrap() else {
        panic!("checkpoint must resume");
    };
    mutation
        .commit("test_result", Some("result"), None, Some(1))
        .await
        .unwrap();
    let MutationStart::Replay(receipt) = MutationEnvelope::resume(
        &rt.store,
        subject,
        operation,
        "test_mutation",
        &digest,
        owner,
    )
    .await
    .unwrap() else {
        panic!("terminal result must replay");
    };
    assert_eq!(receipt.state, "committed");
    assert_eq!(receipt.result_ref.as_deref(), Some("result"));
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), sequence);
    let families: Vec<(String, String, i64)> = sqlx::query_as("SELECT family,space_signature,desired_authority_seq FROM projection_watermarks WHERE subject_id=$1 AND desired_authority_seq=$2 ORDER BY family")
        .bind(subject.0).bind(sequence).fetch_all(rt.store.pool()).await.unwrap();
    assert_eq!(
        families,
        vec![
            ("concept".into(), "*".into(), sequence),
            ("dense".into(), "space-a".into(), sequence),
            ("topology".into(), String::new(), sequence)
        ]
    );
}

async fn envelope_subject(rt: &nous_kernel::NousRuntime) -> SubjectId {
    rt.subjects
        .create_subject(nous_subject::CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: nous_subject::CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            },
            metadata: serde_json::json!({}),
            capabilities: None,
        })
        .await
        .unwrap()
        .subject_id
}
