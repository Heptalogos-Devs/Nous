use nous_domain::*;
use nous_storage::*;
use serde_json::json;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

const DATE: &str = "2026-10-23";
const NOW: &str = "2026-10-23T00:00:00Z";

fn directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/storage");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn key(id: &str) -> OperationKey {
    OperationKey {
        caller_id: "storage-test".into(),
        operation: "test".into(),
        scope: "instance".into(),
        operation_id: id.into(),
        request_hash: "nous.test.v1".into(),
    }
}

async fn transaction(database: &Database) -> WriteTransaction {
    let BeginOperation::New(tx) = database
        .begin_operation(key(&ulid::Ulid::generate().to_string()))
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    tx
}

async fn fixture(database: &Database) -> (FormationRun, ModelCall) {
    let mut tx = transaction(database).await;
    let namespace_id = NamespaceId::new();
    let subject_id = SubjectId::new();
    let material_id = MaterialId::new();
    let observation_id = ObservationId::new();
    tx.insert_namespace(&Namespace {
        id: namespace_id.clone(),
        name: "memory test".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_subject(&Subject {
        id: subject_id.clone(),
        namespace_id: namespace_id.clone(),
        name: "memory subject".into(),
        capabilities: vec![Capability::Memory],
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_material(&Material {
        id: material_id.clone(),
        namespace_id: namespace_id.clone(),
        content_hash: "source-hash".into(),
        media_type: "text/plain".into(),
        byte_len: 4,
        source: Some("source.txt".into()),
        event_time: None,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_observation(&Observation {
        id: observation_id.clone(),
        namespace_id: namespace_id.clone(),
        subject_id: subject_id.clone(),
        material_id: material_id.clone(),
        observed_at: None,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    let run = FormationRun {
        job_id: ulid::Ulid::generate().to_string(),
        namespace_id,
        subject_id,
        observation_id,
        material_id,
        subject_revision: 1,
        material_hash: "source-hash".into(),
        state: FormationState::Queued,
        model_call_id: None,
        memory_ids: vec![],
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    };
    tx.insert_formation(&run).await.unwrap();
    tx.enqueue(JobEnvelope {
        id: run.job_id.clone(),
        kind: "memory.form.v1".into(),
        namespace_id: run.namespace_id.to_string(),
        subject_id: Some(run.subject_id.to_string()),
        expected_revision: Some(1),
        payload: json!({"observation_id":run.observation_id}),
    })
    .await
    .unwrap();
    tx.commit(json!({"queued":run.job_id})).await.unwrap();
    let call = ModelCall {
        id: ModelCallId::new(),
        job_id: run.job_id.clone(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        attempt: 1,
        resource_id: "model-resource".into(),
        request: json!({"schema":"nous.model_request.v1","prompt_version":"test.v1","source_hash":run.material_hash}),
        response: None,
        state: ModelCallState::Started,
        error: None,
        cost_microunits: None,
        created_at: NOW.into(),
        completed_at: None,
    };
    (run, call)
}

fn memory(run: &FormationRun, call: &ModelCall, sequence: i64) -> Memory {
    Memory {
        id: MemoryId::new(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        revision: 1,
        title: "Remembered text".into(),
        content: "A traceable statement.".into(),
        epistemic_status: "observed".into(),
        observation_id: run.observation_id.clone(),
        material_id: run.material_id.clone(),
        model_call_id: call.id.clone(),
        prompt_version: "test.v1".into(),
        times: CognitiveTimes {
            event: None,
            observation: None,
            formation: TimePoint {
                domain: "utc_micros".into(),
                coordinate: 1,
            },
            validity: None,
            recorded: TimePoint {
                domain: "utc_micros".into(),
                coordinate: 1,
            },
        },
        recorded_at: NOW.into(),
        deleted: false,
        commit_seq: sequence,
    }
}

#[tokio::test]
async fn upgrades_m0_preserving_objects_jobs_and_receipts() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let queue = apalis_sqlite::SqliteStorage::migrations();
    let authority = sqlx::migrate!("./migrations");
    let old = queue
        .iter()
        .chain(authority.iter())
        .filter(|m| m.version <= 20261023000000)
        .cloned()
        .collect();
    sqlx::migrate::Migrator::with_migrations(old)
        .run(&pool)
        .await
        .unwrap();
    let id = NamespaceId::new();
    let receipt = OperationReceipt {
        operation_id: "m0-create".into(),
        commit_seq: 1,
        result: json!({"id":id,"name":"existing"}),
        job_ids: vec![],
    };
    sqlx::query("UPDATE authority_state SET commit_seq=1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO namespaces VALUES (?, 'existing', 1, 1)")
        .bind(id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO operation_receipts VALUES ('storage-test','test','instance','m0-create','nous.test.v1',1,?)")
        .bind(serde_json::to_string(&receipt).unwrap()).execute(&pool).await.unwrap();
    let job_id = ulid::Ulid::generate().to_string();
    let envelope = JobEnvelope {
        id: job_id.clone(),
        kind: "m0.pending".into(),
        namespace_id: id.to_string(),
        subject_id: None,
        expected_revision: None,
        payload: json!({"old":true}),
    };
    let task = apalis::prelude::TaskBuilder::new(serde_json::to_vec(&envelope).unwrap())
        .task_id(apalis::prelude::TaskId::from(job_id.clone()))
        .build();
    let mut connection = pool.acquire().await.unwrap();
    apalis_sqlite::queries::push_tasks(&mut *connection, JOB_QUEUE, &[task])
        .await
        .unwrap();
    drop(connection);
    pool.close().await;
    let database = Database::open(&path).await.unwrap();
    assert_eq!(database.namespace(&id).await.unwrap().name, "existing");
    assert_eq!(database.status().await.unwrap().commit_seq, 1);
    assert_eq!(
        database.job(&job_id).await.unwrap().envelope.kind,
        "m0.pending"
    );
    let BeginOperation::Replay(replayed) =
        database.begin_operation(key("m0-create")).await.unwrap()
    else {
        panic!("receipt retained")
    };
    assert_eq!(receipt, replayed);
    assert!(
        database
            .memory_changes(0, 1)
            .await
            .unwrap()
            .memories
            .is_empty()
    );
    database.close().await;
}

#[tokio::test]
async fn revisions_budget_and_commit_batches_are_durable() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let database = Database::open(&path).await.unwrap();
    let (mut run, mut call) = fixture(&database).await;
    let mut tx = transaction(&database).await;
    assert_eq!(
        tx.daily_model_calls(&call.resource_id, DATE).await.unwrap(),
        0
    );
    tx.insert_model_call(&call).await.unwrap();
    assert_eq!(
        tx.daily_model_calls(&call.resource_id, DATE).await.unwrap(),
        1
    );
    call.state = ModelCallState::Uncertain;
    call.error = Some(Error::new(
        ErrorCode::DependencyUnavailable,
        "response was lost",
    ));
    call.completed_at = Some(NOW.into());
    tx.update_model_call(&call).await.unwrap();
    assert_eq!(
        tx.daily_model_calls(&call.resource_id, DATE).await.unwrap(),
        1
    );
    tx.commit(json!({"uncertain":call.id})).await.unwrap();
    let mut succeeded = call.clone();
    succeeded.id = ModelCallId::new();
    succeeded.attempt = 2;
    succeeded.state = ModelCallState::Succeeded;
    succeeded.error = None;
    succeeded.response = Some(
        json!({"schema":"nous.model_response.v1","usage":{"input_tokens":12,"output_tokens":6}}),
    );
    let mut tx = transaction(&database).await;
    tx.insert_model_call(&succeeded).await.unwrap();
    assert_eq!(
        tx.daily_model_calls(&call.resource_id, DATE).await.unwrap(),
        2
    );
    let original = memory(&run, &succeeded, tx.commit_seq());
    let second = memory(&run, &succeeded, tx.commit_seq());
    tx.save_memory(&original, None).await.unwrap();
    tx.save_memory(&second, None).await.unwrap();
    run.state = FormationState::Succeeded;
    run.model_call_id = Some(succeeded.id.clone());
    run.memory_ids = vec![original.id.clone(), second.id.clone()];
    tx.update_formation(&run).await.unwrap();
    tx.commit(json!({"adopted":run.memory_ids})).await.unwrap();
    let mut tx = transaction(&database).await;
    let mut deleted = original.clone();
    deleted.revision = 2;
    deleted.deleted = true;
    deleted.commit_seq = tx.commit_seq();
    tx.save_memory(&deleted, Some(1)).await.unwrap();
    tx.commit(json!({"deleted":deleted.id})).await.unwrap();
    let batch = database.memory_changes(0, 1).await.unwrap();
    assert!(batch.has_more);
    assert_eq!(batch.memories.len(), 2);
    assert_eq!(batch.through_commit_seq, original.commit_seq);
    let next = database
        .memory_changes(batch.through_commit_seq, 1)
        .await
        .unwrap();
    assert!(!next.has_more);
    assert_eq!(next.memories.len(), 1);
    assert!(next.memories[0].deleted);
    assert_eq!(
        database
            .memory_history(&run.namespace_id, &original.id)
            .await
            .unwrap()
            .len(),
        2
    );
    let unrelated = database
        .subject_memory_changes(&run.namespace_id, &SubjectId::new(), 0, 1)
        .await
        .unwrap();
    assert!(unrelated.memories.is_empty());
    assert!(!unrelated.has_more);
    let mut tx = transaction(&database).await;
    let stale = Memory {
        commit_seq: tx.commit_seq(),
        ..deleted.clone()
    };
    assert_eq!(
        tx.save_memory(&stale, Some(1)).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&database).await;
    let changed = ModelCall {
        request: json!({"changed":true}),
        ..succeeded.clone()
    };
    assert_eq!(
        tx.update_model_call(&changed).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    database.close().await;
    let database = Database::open(&path).await.unwrap();
    assert!(
        database
            .memory(&run.namespace_id, &original.id)
            .await
            .unwrap()
            .deleted
    );
    assert_eq!(
        database
            .formation(&run.job_id)
            .await
            .unwrap()
            .memory_ids
            .len(),
        2
    );
    assert_eq!(database.model_calls(&run.job_id).await.unwrap().len(), 2);
    assert!(
        database
            .model_call(&succeeded.id)
            .await
            .unwrap()
            .cost_microunits
            .is_none()
    );
    database.close().await;
}

#[tokio::test]
async fn waiting_resume_and_startup_recovery_preserve_formation_outcomes() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let database = Database::open(&path).await.unwrap();
    let (mut waiting, call) = fixture(&database).await;
    let (mut completed, _) = fixture(&database).await;
    let (mut active_waiting, _) = fixture(&database).await;
    let mut tx = transaction(&database).await;
    waiting.state = FormationState::WaitingForBudget;
    completed.state = FormationState::Succeeded;
    active_waiting.state = FormationState::WaitingForConfiguration;
    tx.update_formation(&waiting).await.unwrap();
    tx.update_formation(&completed).await.unwrap();
    tx.update_formation(&active_waiting).await.unwrap();
    tx.insert_model_call(&call).await.unwrap();
    tx.commit(json!({"fixture":true})).await.unwrap();
    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    sqlx::query("INSERT INTO Workers(id,worker_type,storage_name,last_seen,started_at) VALUES ('departed',?,'SqliteStorage',strftime('%s','now'),strftime('%s','now'))")
        .bind(JOB_QUEUE).execute(&pool).await.unwrap();
    sqlx::query("UPDATE Jobs SET status='Done',lock_by='departed' WHERE id=?")
        .bind(&waiting.job_id)
        .execute(&pool)
        .await
        .unwrap();
    for id in [&completed.job_id, &active_waiting.job_id] {
        sqlx::query("UPDATE Jobs SET status='Running',lock_by='departed' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert_eq!(database.resume_waiting_formations().await.unwrap(), 1);
    assert_eq!(
        database.formation(&waiting.job_id).await.unwrap().state,
        FormationState::Queued
    );
    assert_eq!(
        database
            .formation(&active_waiting.job_id)
            .await
            .unwrap()
            .state,
        FormationState::WaitingForConfiguration
    );
    assert_eq!(database.recover_interrupted_jobs().await.unwrap(), 2);
    assert_eq!(
        database.formation(&completed.job_id).await.unwrap().state,
        FormationState::Succeeded
    );
    assert_eq!(
        database.model_call(&call.id).await.unwrap().state,
        ModelCallState::Started
    );
    assert_eq!(
        database.job(&completed.job_id).await.unwrap().status,
        "Pending"
    );
    assert_eq!(database.resume_waiting_formations().await.unwrap(), 0);
    assert_eq!(database.recover_interrupted_jobs().await.unwrap(), 0);
    let worker_count: i64 = sqlx::query_scalar("SELECT count(*) FROM Workers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(worker_count, 0);
    pool.close().await;
    database.close().await;
}
