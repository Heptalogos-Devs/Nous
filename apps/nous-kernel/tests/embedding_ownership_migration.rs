// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::*;
use nous_kernel::NousRuntime;
use nous_persistence::AuthorityStore;
use nous_retrieval::{StoredEmbeddingConfig, TextEmbeddingOrigin, TextEmbeddingRequest};
use test_support::{database, form_input};

fn stored_embedding_config() -> StoredEmbeddingConfig {
    let mut space = EmbeddingSpaceSignature {
        space_hash: String::new(),
        model_identity: "legacy-upgrade-test".into(),
        weights_revision: "1".into(),
        task: "text_embedding".into(),
        input_representation: "text".into(),
        preprocessing_identity: "identity".into(),
        preprocessing_revision: "1".into(),
        dimension: 2,
        normalization: "l2".into(),
        output_semantics: "test-vector".into(),
    };
    space.space_hash = blake3::hash(&serde_json::to_vec(&space).unwrap())
        .to_hex()
        .to_string();
    let producer = AuthorityStore::canonical_producer(&ProducerSignature {
        model_role: None,
        model_profile: None,
        execution_profile: None,
        inference_controls_digest: None,
        role_policy_digest: None,
        prompt_id: None,
        prompt_digest: None,
        signature_hash: String::new(),
        provider_class: "stored-embedding-test".into(),
        operation: CapabilityOperation::TextEmbedding,
        implementation: "stored-embedding-test".into(),
        model_identity: Some(space.model_identity.clone()),
        model_revision: Some(space.weights_revision.clone()),
        output_schema_digest: None,
        preprocessing_identity: space.preprocessing_identity.clone(),
        preprocessing_revision: space.preprocessing_revision.clone(),
        config_digest: "test".into(),
    })
    .unwrap();
    StoredEmbeddingConfig {
        space,
        producers: vec![producer],
    }
}

async fn create_memory(
    runtime: &NousRuntime,
    subject: SubjectId,
    occurrence: OccurrenceId,
    text: &str,
) -> nous_memory::MemoryView {
    runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(subject, occurrence, OperationId::new(), text))
        .await
        .unwrap()
}

#[tokio::test]
async fn copied_upgrade_binds_known_vectors_backs_up_unknown_and_purge_keeps_backup() {
    let (root, url, _postgres) = database().await;
    let runtime = test_support::open_memory_only_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let source = runtime
        .material
        .record_observation(test_support::longitudinal::observation(subject, None))
        .await
        .unwrap();
    let text = "same legacy memory text";
    let first = create_memory(&runtime, subject, source.occurrence.occurrence_id, text).await;
    let second = create_memory(&runtime, subject, source.occurrence.occurrence_id, text).await;
    let first_ref = CognitiveRef::MemoryRevision(first.revision.memory_revision_id);
    let config = stored_embedding_config();
    runtime
        .serving
        .initialize_embedding(config.clone())
        .unwrap();

    let digest = nous_material::text_content_identity(text);
    let unknown_digest = nous_material::text_content_identity("legacy source no longer readable");
    sqlx::query("INSERT INTO embedding_materials(subject_id,content_digest,space_hash,producer_hash,vector) VALUES($1,$2,$3,$4,$5),($1,$6,$3,$4,$7)")
        .bind(subject.0)
        .bind(&digest)
        .bind(&config.space.space_hash)
        .bind(&config.producers[0].signature_hash)
        .bind(vec![1.0f32, 0.0])
        .bind(&unknown_digest)
        .bind(vec![0.0f32, 1.0])
        .execute(runtime.store.pool())
        .await
        .unwrap();

    let provider = runtime.serving.embedding().unwrap();
    let request = TextEmbeddingRequest {
        subject,
        text: text.into(),
        origin: TextEmbeddingOrigin::Document(first_ref.clone()),
    };
    assert!(provider.embed(request.clone()).await.is_err());
    assert!(
        runtime
            .serving
            .commit_embedding(
                subject,
                vec![first_ref.clone()],
                text.into(),
                &config.space.space_hash,
                &config.producers[0].signature_hash,
                vec![0.0, 1.0],
            )
            .await
            .is_err()
    );

    let report = runtime
        .serving
        .migrate_legacy_embedding_uses()
        .await
        .unwrap();
    assert_eq!(report.legacy_rows_examined, 2);
    assert_eq!(report.recovered_rows, 1);
    assert_eq!(report.backed_up_rows, 1);
    assert_eq!(
        provider.embed(request).await.unwrap().vector,
        vec![1.0, 0.0]
    );
    let bindings: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embedding_material_uses WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(bindings, 2);
    let active_unknown: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embedding_materials WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&unknown_digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    let backed_up: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM legacy_embedding_materials_backup WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&unknown_digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(active_unknown, 0);
    assert_eq!(backed_up, 1);

    let stale_historical_view = runtime
        .historical_authority_view(
            subject,
            chrono::Utc::now() + chrono::Duration::seconds(1),
            RevisionView::History,
        )
        .await
        .unwrap();
    let first_purge_operation = OperationId::new();
    runtime
        .require_memory()
        .unwrap()
        .begin_purge_memory(
            subject,
            first.object.memory_id,
            first_purge_operation,
            first.object.object_epoch,
        )
        .await
        .unwrap();
    assert!(
        runtime
            .serving
            .commit_embedding_in_view(
                subject,
                vec![first_ref.clone()],
                text.into(),
                &config.space.space_hash,
                &config.producers[0].signature_hash,
                vec![1.0, 0.0],
                Some(&stale_historical_view),
            )
            .await
            .is_err()
    );
    runtime
        .purge_memory(
            subject,
            first.object.memory_id,
            first_purge_operation,
            first.object.object_epoch,
        )
        .await
        .unwrap();
    let after_first_purge: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embedding_materials WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(after_first_purge, 1);
    runtime
        .purge_memory(
            subject,
            second.object.memory_id,
            OperationId::new(),
            second.object.object_epoch,
        )
        .await
        .unwrap();
    let after_second_purge: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embedding_materials WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(after_second_purge, 0);
    let remaining_uses: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM embedding_material_uses WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(remaining_uses, 0);
    let backup_remains: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM legacy_embedding_materials_backup WHERE subject_id=$1 AND content_digest=$2",
    )
    .bind(subject.0)
    .bind(&unknown_digest)
    .fetch_one(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(backup_remains, 1);
}
