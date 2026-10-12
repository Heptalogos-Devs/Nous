// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_persistence::{
    DenseInvalidation, MutationEnvelope, MutationStart, OwnerLock, ProjectionInvalidation,
};

use nous_core::{OperationId, SubjectId};

mod test_support;

#[tokio::test]
async fn mutation_envelope_rolls_back_merges_families_and_replays() {
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
    mutation
        .commit("test_result", Some("result"), None, Some(1))
        .await
        .unwrap();
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
        panic!("terminal result must replay");
    };
    assert!(
        matches!(MutationEnvelope::begin(&rt.store, subject, operation, "test_mutation", "different", owner).await,
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::OperationIdConflict)
    );
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

#[tokio::test]
async fn seed_adoption_shares_artifact_registration_and_rolls_back_on_failure() {
    use nous_material::{ObservationMaterial, UploadMetadata};
    use nous_subject::{COGNITIVE_SEED_FORMAT, CognitiveSeedInput, SeedAdoptionKind};
    let (root, url, _postgres) = test_support::database().await;
    let rt = test_support::open_runtime(&url, &root).await;
    let subject = envelope_subject(&rt).await;
    let original = rt.subjects.latest_cognitive_seed(subject).await.unwrap();
    let artifact = rt
        .material
        .artifact(subject, original.version.artifact_id)
        .await
        .unwrap();
    assert_eq!(
        artifact.storage_key,
        nous_object_store::ObjectStore::storage_key(&artifact.content_hash).unwrap()
    );
    let uploaded = rt
        .material
        .upload_stream(
            subject,
            UploadMetadata {
                media_type: "text/plain".into(),
                metadata: serde_json::json!({"later": true}),
            },
            futures::stream::iter([Ok(original.text.as_bytes().to_vec())]),
        )
        .await
        .unwrap();
    let observed = test_support::observation(&rt, subject, &original.text).await;
    let observed_artifact = observed.artifact.unwrap();
    assert_eq!(uploaded.artifact_id, artifact.artifact_id);
    assert_eq!(observed_artifact.artifact_id, artifact.artifact_id);
    assert_eq!(uploaded.media_type, artifact.media_type);
    assert_eq!(observed_artifact.media_type, artifact.media_type);
    assert_eq!(uploaded.metadata, artifact.metadata);
    assert_eq!(observed_artifact.storage_key, artifact.storage_key);
    let operation = OperationId::new();
    let seed = CognitiveSeedInput {
        text: original.text.clone(),
        format: COGNITIVE_SEED_FORMAT.into(),
        provenance: serde_json::json!({"import": true}),
    };
    let imported = rt
        .subjects
        .adopt_cognitive_seed(subject, operation, seed.clone(), SeedAdoptionKind::Import)
        .await
        .unwrap();
    let replay = rt
        .subjects
        .adopt_cognitive_seed(subject, operation, seed, SeedAdoptionKind::Import)
        .await
        .unwrap();
    assert_eq!(imported.version.artifact_id, artifact.artifact_id);
    assert_eq!(
        imported.version.seed_version_id,
        original.version.seed_version_id
    );
    assert_eq!(imported.adoption.adoption_id, replay.adoption.adoption_id);
    assert_ne!(original.adoption.adoption_id, imported.adoption.adoption_id);
    let before = rt.store.authority_seq(subject).await.unwrap();
    // Reject the final adoption insert after CAS, Artifact and version work have run.
    sqlx::raw_sql("CREATE FUNCTION reject_seed_adoption() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test adoption failure'; END $$; CREATE TRIGGER reject_seed_adoption BEFORE INSERT ON subject_seed_adoptions FOR EACH ROW EXECUTE FUNCTION reject_seed_adoption();")
        .execute(rt.store.pool()).await.unwrap();
    let failed_text = "schema_version = 1\nname = 'rollback'";
    assert!(
        rt.subjects
            .adopt_cognitive_seed(
                subject,
                OperationId::new(),
                CognitiveSeedInput {
                    text: failed_text.into(),
                    format: COGNITIVE_SEED_FORMAT.into(),
                    provenance: serde_json::json!({}),
                },
                SeedAdoptionKind::Import
            )
            .await
            .is_err()
    );
    let hash = blake3::hash(failed_text.as_bytes()).to_hex().to_string();
    let absent: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM artifacts WHERE subject_id=$1 AND content_hash=$2),(SELECT count(*) FROM cognitive_seed_versions WHERE subject_id=$1 AND content_hash=$2)")
        .bind(subject.0).bind(&hash).fetch_one(rt.store.pool()).await.unwrap();
    assert_eq!(absent, (0, 0));
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);
    assert_eq!(
        rt.subjects
            .cognitive_seed(subject, imported.adoption.adoption_id)
            .await
            .unwrap()
            .text,
        original.text
    );
    // Observation admission rolls Artifact work back with a later semantic failure too.
    let mut input = test_support::longitudinal::observation(subject, None);
    input.material = ObservationMaterial::InlineText {
        text: failed_text.into(),
        media_type: "text/plain".into(),
    };
    input.entities.push(nous_material::ResolvedEntityMention {
        surface: "".into(),
        semantic_role: None,
        entity_ref: None,
    });
    assert!(rt.material.record_observation(input).await.is_err());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artifacts WHERE subject_id=$1 AND content_hash=$2",
    )
    .bind(subject.0)
    .bind(hash)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
}
