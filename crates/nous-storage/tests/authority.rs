use nous_domain::*;
use nous_storage::*;
use serde_json::json;
const NOW: &str = "2026-10-23T00:00:00Z";
fn directory() -> tempfile::TempDir {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/storage");
    std::fs::create_dir_all(&path).unwrap();
    tempfile::tempdir_in(path).unwrap()
}
fn key(id: &str) -> OperationKey {
    OperationKey {
        caller_id: "test".into(),
        operation: "write".into(),
        scope: "instance".into(),
        operation_id: id.into(),
        request_hash: "request".into(),
    }
}
async fn transaction(db: &Database) -> WriteTransaction {
    let BeginOperation::New(tx) = db
        .begin_operation(key(&ulid::Ulid::generate().to_string()))
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    tx
}
fn task(kind: &str) -> JobRecord {
    JobRecord {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: None,
        subject_id: None,
        kind: kind.into(),
        state: JobState::Queued,
        wait: None,
        checkpoint: None,
        result: None,
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    }
}

#[tokio::test]
async fn operation_receipt_task_and_snapshot_share_real_authority() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let job = task("backup.create");
    let mut tx = transaction(&db).await;
    tx.insert_job_record(&job).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        db.job_record(&job.id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(db.status().await.unwrap().commit_seq, 0);
    let ns = NamespaceId::new();
    let mut view = db.read_view().await.unwrap();
    let mut tx = transaction(&db).await;
    tx.insert_namespace(&Namespace {
        id: ns.clone(),
        name: "one instance".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_job_record(&job).await.unwrap();
    tx.commit(json!({"id":ns})).await.unwrap();
    assert_eq!(
        view.namespace(&ns).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    view.finish().await.unwrap();
    assert_eq!(db.namespace(&ns).await.unwrap().name, "one instance");
    assert_eq!(db.queue_record(&job.id).await.unwrap().status, "Pending");
    let mut writers = Vec::new();
    for _ in 0..2 {
        let db = db.clone();
        writers.push(tokio::spawn(async move {
            match db.begin_operation(key("same-write")).await.unwrap() {
                BeginOperation::New(tx) => tx.commit(json!({"accepted":true})).await.unwrap(),
                BeginOperation::Replay(receipt) => receipt,
            }
        }));
    }
    let first = writers.remove(0).await.unwrap();
    let second = writers.remove(0).await.unwrap();
    assert_eq!(first, second);
    let mut changed = key("same-write");
    changed.request_hash = "changed".into();
    assert_eq!(
        db.begin_operation(changed).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    let snapshot = dir.path().join("snapshot.sqlite");
    db.backup_to(&snapshot).await.unwrap();
    db.close().await;
    let restored = Database::open(snapshot).await.unwrap();
    assert_eq!(
        restored.job_record(&job.id).await.unwrap().kind,
        "backup.create"
    );
    assert_eq!(restored.namespace(&ns).await.unwrap().id, ns);
    restored.close().await;
}

#[tokio::test]
async fn generic_wait_conditions_requeue_only_after_ack() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let mut job = task("imports.text");
    let mut tx = transaction(&db).await;
    tx.insert_job_record(&job).await.unwrap();
    tx.commit(json!({"job_id":job.id})).await.unwrap();
    job.state = JobState::Waiting;
    job.wait = Some(WaitCondition {
        kind: "operator".into(),
        details: json!({"action":"approve"}),
    });
    let mut tx = transaction(&db).await;
    tx.update_job_record(&job).await.unwrap();
    tx.commit(json!({"waiting":true})).await.unwrap();
    assert_eq!(db.waiting_jobs().await.unwrap().len(), 1);
    let mut queued = job.clone();
    queued.state = JobState::Queued;
    queued.wait = None;
    let mut tx = transaction(&db).await;
    assert_eq!(
        tx.update_job_record(&queued).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    let pool =
        sqlx::SqlitePool::connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
    sqlx::query("UPDATE Jobs SET status='Done' WHERE id=?")
        .bind(&job.id)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = transaction(&db).await;
    tx.update_job_record(&queued).await.unwrap();
    tx.commit(json!({"resumed":true})).await.unwrap();
    assert_eq!(db.queue_record(&job.id).await.unwrap().status, "Pending");
    assert!(db.waiting_jobs().await.unwrap().is_empty());
    pool.close().await;
    db.close().await;
}
