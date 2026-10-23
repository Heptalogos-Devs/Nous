use nous_domain::*;
use nous_storage::*;
use serde_json::json;

#[tokio::test]
async fn sqlite_snapshot_retains_object_set_receipts_and_accepted_jobs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/storage");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let source = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let operation = OperationKey {
        caller_id: "backup-test".into(),
        operation: "fixture".into(),
        scope: "instance".into(),
        operation_id: "snapshot-fixture".into(),
        request_hash: "nous.backup.fixture.v1".into(),
    };
    let BeginOperation::New(mut tx) = source.begin_operation(operation.clone()).await.unwrap()
    else {
        panic!("new operation")
    };
    let namespace_id = NamespaceId::new();
    tx.insert_namespace(&Namespace {
        id: namespace_id.clone(),
        name: "backup source".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    for (hash, length) in [("hash-a", 7), ("hash-a", 7), ("hash-b", 11)] {
        tx.insert_material(&Material {
            id: MaterialId::new(),
            namespace_id: namespace_id.clone(),
            content_hash: hash.into(),
            media_type: "text/plain".into(),
            byte_len: length,
            source: None,
            event_time: None,
            recorded_at: "2026-10-23T00:00:00Z".into(),
            commit_seq: tx.commit_seq(),
        })
        .await
        .unwrap();
    }
    let job_id = ulid::Ulid::generate().to_string();
    tx.enqueue(JobEnvelope {
        id: job_id.clone(),
        kind: "snapshot.pending".into(),
        namespace_id: namespace_id.to_string(),
        subject_id: None,
        expected_revision: None,
        payload: json!({"accepted":true}),
    })
    .await
    .unwrap();
    let receipt = tx.commit(json!({"fixture":true})).await.unwrap();
    // The apostrophe exercises bound path parameters, including on Windows.
    let snapshot_path = directory.path().join("backup 'quoted'.sqlite");
    source.backup_to(&snapshot_path).await.unwrap();
    let original = std::fs::read(&snapshot_path).unwrap();
    assert_eq!(
        source.backup_to(&snapshot_path).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(original, std::fs::read(&snapshot_path).unwrap());
    assert_eq!(
        source.backup_to("relative.sqlite").await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let BeginOperation::New(mut tx) = source
        .begin_operation(OperationKey {
            operation_id: "after-snapshot".into(),
            ..operation.clone()
        })
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    tx.insert_namespace(&Namespace {
        id: NamespaceId::new(),
        name: "after backup".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.commit(json!({"after":true})).await.unwrap();
    let snapshot = Database::open(&snapshot_path).await.unwrap();
    assert_eq!(
        snapshot.status().await.unwrap().commit_seq,
        receipt.commit_seq
    );
    assert_eq!(snapshot.list_namespaces().await.unwrap().len(), 1);
    assert_eq!(
        snapshot.namespace(&namespace_id).await.unwrap().name,
        "backup source"
    );
    assert_eq!(
        snapshot.object_references().await.unwrap(),
        vec![
            ObjectReference {
                content_hash: "hash-a".into(),
                byte_len: 7
            },
            ObjectReference {
                content_hash: "hash-b".into(),
                byte_len: 11
            }
        ]
    );
    assert_eq!(snapshot.job(&job_id).await.unwrap().status, "Pending");
    let BeginOperation::Replay(replayed) = snapshot.begin_operation(operation).await.unwrap()
    else {
        panic!("receipt retained")
    };
    assert_eq!(receipt, replayed);
    snapshot.close().await;
    source.close().await;
}
