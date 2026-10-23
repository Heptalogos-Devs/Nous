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
        kind: ObservationKind::Contact,
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
        model_call_id: Some(call.id.clone()),
        prompt_version: Some("test.v1".into()),
        origin: MemoryOrigin::Model,
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
        lifecycle: MemoryLifecycle::new(1),
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
    let (mut settled_configuration, _) = fixture(&database).await;
    let mut tx = transaction(&database).await;
    waiting.state = FormationState::WaitingForBudget;
    completed.state = FormationState::Succeeded;
    active_waiting.state = FormationState::WaitingForConfiguration;
    settled_configuration.state = FormationState::WaitingForConfiguration;
    tx.update_formation(&waiting).await.unwrap();
    tx.update_formation(&completed).await.unwrap();
    tx.update_formation(&active_waiting).await.unwrap();
    tx.update_formation(&settled_configuration).await.unwrap();
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
    for id in [&waiting.job_id, &settled_configuration.job_id] {
        sqlx::query("UPDATE Jobs SET status='Done',lock_by='departed' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    for id in [&completed.job_id, &active_waiting.job_id] {
        sqlx::query("UPDATE Jobs SET status='Running',lock_by='departed' WHERE id=?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert_eq!(database.resume_waiting_configurations().await.unwrap(), 1);
    assert_eq!(
        database
            .formation(&settled_configuration.job_id)
            .await
            .unwrap()
            .state,
        FormationState::Queued
    );
    assert_eq!(
        database.formation(&waiting.job_id).await.unwrap().state,
        FormationState::WaitingForBudget
    );
    let unchanged_sequence = database.status().await.unwrap().commit_seq;
    assert_eq!(database.resume_waiting_configurations().await.unwrap(), 0);
    assert_eq!(
        database.status().await.unwrap().commit_seq,
        unchanged_sequence
    );
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

#[tokio::test]
async fn lifecycle_revisions_audit_and_source_metadata_share_the_commit() {
    let directory = directory();
    let database = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let (run, mut call) = fixture(&database).await;
    call.state = ModelCallState::Succeeded;
    let mut tx = transaction(&database).await;
    tx.insert_model_call(&call).await.unwrap();
    let original = memory(&run, &call, tx.commit_seq());
    tx.save_memory(&original, None).await.unwrap();
    tx.commit(serde_json::to_value(&original).unwrap())
        .await
        .unwrap();
    assert_eq!(
        database
            .material_details(&run.namespace_id, &run.material_id)
            .await
            .unwrap()
            .metadata,
        json!({})
    );
    let details = MaterialDetails {
        source_type: Some("lpmm_openie".into()),
        tags: vec!["tag".into()],
        metadata: json!({"import_format":"jsonl"}),
        structured: Some(json!({"entities":["subject"]})),
    };
    let mut tx = transaction(&database).await;
    let mut frozen = original.clone();
    frozen.revision = 2;
    frozen.commit_seq = tx.commit_seq();
    frozen.lifecycle.activation = MemoryActivation::Frozen;
    frozen.lifecycle.strength = 0.6;
    frozen.lifecycle.inactive_since_unix_ms = Some(2);
    tx.save_memory(&frozen, Some(1)).await.unwrap();
    tx.set_material_details(&run.namespace_id, &run.material_id, &details)
        .await
        .unwrap();
    tx.append_memory_audit(&MemoryAudit {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        memory_id: original.id.clone(),
        action: "freeze".into(),
        actor: "test".into(),
        reason: "manual".into(),
        previous_revision: 1,
        revision: 2,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
        details: json!({"strength":0.6}),
    })
    .await
    .unwrap();
    tx.commit(json!({"revision":2})).await.unwrap();
    let history = database
        .memory_history(&run.namespace_id, &original.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].lifecycle.activation, MemoryActivation::Active);
    assert_eq!(history[1].lifecycle.activation, MemoryActivation::Frozen);
    assert_eq!(
        database
            .maintenance_page(&run.namespace_id, &run.subject_id, None, None, 50)
            .await
            .unwrap()
            .memories[0]
            .lifecycle
            .strength,
        0.6
    );
    assert!(
        database
            .recycle_bin(&run.namespace_id, &run.subject_id, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        database
            .memory_audit(&run.namespace_id, &original.id, 50)
            .await
            .unwrap()[0]
            .revision,
        2
    );
    let retained = database
        .material_details(&run.namespace_id, &run.material_id)
        .await
        .unwrap();
    assert_eq!(retained.tags, details.tags);
    assert_eq!(retained.structured, details.structured);
    assert_eq!(
        database
            .material_details(&NamespaceId::new(), &run.material_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    database.close().await;
}

#[tokio::test]
async fn permanent_purge_is_atomic_redacts_copies_and_invalidates_indexes() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let database = Database::open(&path).await.unwrap();
    let (mut run, mut call) = fixture(&database).await;
    call.state = ModelCallState::Succeeded;
    call.response = Some(
        json!({"record":{"raw_output":"memory-only secret","provider_response":{"text":"secret"}},"memories":[{"content":"memory-only secret"}]}),
    );
    let mut tx = transaction(&database).await;
    tx.insert_model_call(&call).await.unwrap();
    let original = memory(&run, &call, tx.commit_seq());
    let shared = memory(&run, &call, tx.commit_seq());
    tx.save_memory(&original, None).await.unwrap();
    tx.save_memory(&shared, None).await.unwrap();
    run.state = FormationState::Succeeded;
    run.model_call_id = Some(call.id.clone());
    run.memory_ids = vec![original.id.clone(), shared.id.clone()];
    tx.update_formation(&run).await.unwrap();
    let create_receipt = tx
        .commit(serde_json::to_value(&original).unwrap())
        .await
        .unwrap();
    let mut tx = transaction(&database).await;
    assert_eq!(
        tx.purge_memory(&run.namespace_id, &original.id, 1)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&database).await;
    let mut deleted = original.clone();
    deleted.revision = 2;
    deleted.deleted = true;
    deleted.commit_seq = tx.commit_seq();
    tx.save_memory(&deleted, Some(1)).await.unwrap();
    tx.commit(serde_json::to_value(&deleted).unwrap())
        .await
        .unwrap();
    assert_eq!(
        database
            .recycle_bin(&run.namespace_id, &run.subject_id, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut tx = transaction(&database).await;
    tx.purge_memory(&run.namespace_id, &original.id, 2)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        database
            .memory_history(&run.namespace_id, &original.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        database.model_call(&call.id).await.unwrap().response,
        call.response
    );
    let mut tx = transaction(&database).await;
    let marker = tx
        .purge_memory(&run.namespace_id, &original.id, 2)
        .await
        .unwrap();
    assert_eq!(marker.revision, 3);
    tx.append_memory_audit(&MemoryAudit {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        memory_id: original.id.clone(),
        action: "purge".into(),
        actor: "test".into(),
        reason: "remove derived memory".into(),
        previous_revision: 2,
        revision: marker.revision,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
        details: json!({"purged":true}),
    })
    .await
    .unwrap();
    tx.commit(serde_json::to_value(&marker).unwrap())
        .await
        .unwrap();
    assert_eq!(
        database.memory_namespace(&original.id).await.unwrap(),
        run.namespace_id
    );
    assert_eq!(
        database
            .memory(&run.namespace_id, &original.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        database
            .memory_history(&run.namespace_id, &original.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        database
            .memory(&run.namespace_id, &shared.id)
            .await
            .unwrap()
            .content,
        shared.content
    );
    assert_eq!(
        database
            .material(&run.namespace_id, &run.material_id)
            .await
            .unwrap()
            .content_hash,
        run.material_hash
    );
    assert_eq!(
        database
            .model_call(&call.id)
            .await
            .unwrap()
            .response
            .unwrap(),
        json!({"schema_version":1,"redacted":true,"reason":"memory_purged"})
    );
    assert_eq!(
        database.model_call(&call.id).await.unwrap().request,
        call.request
    );
    assert_eq!(
        database.formation(&run.job_id).await.unwrap().memory_ids,
        vec![shared.id.clone()]
    );
    let changes = database
        .memory_changes(deleted.commit_seq, 1)
        .await
        .unwrap();
    assert_eq!(changes.purged.len(), 1);
    assert_eq!(changes.purged[0].id, original.id);
    assert!(changes.memories.is_empty());
    assert_eq!(changes.through_commit_seq, marker.commit_seq);
    assert_eq!(
        database
            .subject_memory_changes(&run.namespace_id, &SubjectId::new(), 0, 1)
            .await
            .unwrap()
            .purged
            .len(),
        0
    );
    let pool = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    let prior: String =
        sqlx::query_scalar("SELECT receipt FROM operation_receipts WHERE operation_id=?")
            .bind(&create_receipt.operation_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let sanitized: OperationReceipt = serde_json::from_str(&prior).unwrap();
    assert_eq!(sanitized.result, json!({"id":original.id,"purged":true}));
    assert_eq!(sanitized.commit_seq, create_receipt.commit_seq);
    pool.close().await;
    let mut tx = transaction(&database).await;
    let attempted = Memory {
        commit_seq: tx.commit_seq(),
        ..original.clone()
    };
    assert_eq!(
        tx.save_memory(&attempted, None).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    database.close().await;
    let database = Database::open(&path).await.unwrap();
    assert_eq!(
        database
            .purged_memory(&run.namespace_id, &original.id)
            .await
            .unwrap()
            .revision,
        3
    );
    assert_eq!(
        database
            .memory_audit(&run.namespace_id, &original.id, 50)
            .await
            .unwrap()[0]
            .action,
        "purge"
    );
    database.close().await;
}

#[tokio::test]
async fn m1_snapshots_gain_lifecycle_anchors_without_changing_content() {
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
        .filter(|m| m.version <= 20261023010000)
        .cloned()
        .collect();
    sqlx::migrate::Migrator::with_migrations(old)
        .run(&pool)
        .await
        .unwrap();
    let ns = NamespaceId::new();
    let subject = SubjectId::new();
    let material = MaterialId::new();
    let observation = ObservationId::new();
    let call = ModelCallId::new();
    let job = ulid::Ulid::generate().to_string();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("UPDATE authority_state SET commit_seq=1")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO namespaces VALUES (?,'old namespace',1,1)")
        .bind(ns.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO subjects VALUES (?,?,'old subject','[\"memory\"]',1,1)")
        .bind(subject.as_str())
        .bind(ns.as_str())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("INSERT INTO materials(id,namespace_id,content_hash,media_type,byte_len,recorded_at,commit_seq) VALUES (?,?,'old-hash','text/plain',4,?,1)").bind(material.as_str()).bind(ns.as_str()).bind(NOW).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO observations(id,namespace_id,subject_id,material_id,recorded_at,commit_seq) VALUES (?,?,?,?,?,1)").bind(observation.as_str()).bind(ns.as_str()).bind(subject.as_str()).bind(material.as_str()).bind(NOW).execute(&mut *tx).await.unwrap();
    let envelope = JobEnvelope {
        id: job.clone(),
        kind: "memory.form".into(),
        namespace_id: ns.to_string(),
        subject_id: Some(subject.to_string()),
        expected_revision: Some(1),
        payload: json!({}),
    };
    let task = apalis::prelude::TaskBuilder::new(serde_json::to_vec(&envelope).unwrap())
        .task_id(apalis::prelude::TaskId::from(job.clone()))
        .build();
    apalis_sqlite::queries::push_tasks(&mut *tx, JOB_QUEUE, &[task])
        .await
        .unwrap();
    sqlx::query("INSERT INTO formation_runs(job_id,namespace_id,subject_id,observation_id,material_id,subject_revision,material_hash,state,created_at,updated_at) VALUES (?,?,?,?,?,1,'old-hash','succeeded',?,?)").bind(&job).bind(ns.as_str()).bind(subject.as_str()).bind(observation.as_str()).bind(material.as_str()).bind(NOW).bind(NOW).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO model_calls(id,job_id,namespace_id,subject_id,attempt,resource_id,request,state,created_at) VALUES (?,?,?,?,1,'old-model','{}','succeeded',?)").bind(call.as_str()).bind(&job).bind(ns.as_str()).bind(subject.as_str()).bind(NOW).execute(&mut *tx).await.unwrap();
    let mut ids = vec![];
    for (domain, coordinate) in [("unix_ms", 1234), ("world-clock", 42)] {
        let id = MemoryId::new();
        sqlx::query("INSERT INTO memories VALUES (?,?,?,1,1)")
            .bind(id.as_str())
            .bind(ns.as_str())
            .bind(subject.as_str())
            .execute(&mut *tx)
            .await
            .unwrap();
        let times = CognitiveTimes {
            event: None,
            observation: None,
            formation: TimePoint {
                domain: domain.into(),
                coordinate,
            },
            validity: None,
            recorded: TimePoint {
                domain: "unix_ms".into(),
                coordinate: 1792713600000,
            },
        };
        sqlx::query("INSERT INTO memory_revisions(memory_id,namespace_id,subject_id,revision,title,content,epistemic_status,observation_id,material_id,model_call_id,prompt_version,times,recorded_at,deleted,commit_seq) VALUES (?,?,?,1,'existing title','existing text','observed',?,?,?,'old.v1',?,?,0,1)")
            .bind(id.as_str()).bind(ns.as_str()).bind(subject.as_str()).bind(observation.as_str()).bind(material.as_str()).bind(call.as_str()).bind(serde_json::to_string(&times).unwrap()).bind(NOW).execute(&mut *tx).await.unwrap();
        ids.push(id);
    }
    tx.commit().await.unwrap();
    pool.close().await;
    let database = Database::open(&path).await.unwrap();
    assert_eq!(
        database.memory(&ns, &ids[0]).await.unwrap().lifecycle,
        MemoryLifecycle::new(1234)
    );
    assert_eq!(
        database.memory(&ns, &ids[1]).await.unwrap().lifecycle,
        MemoryLifecycle::new(1792713600000)
    );
    assert_eq!(
        database.memory(&ns, &ids[0]).await.unwrap().content,
        "existing text"
    );
    assert_eq!(database.status().await.unwrap().commit_seq, 1);
    assert_eq!(
        database.observation(&ns, &observation).await.unwrap().kind,
        ObservationKind::Contact
    );
    database.close().await;
}

#[tokio::test]
async fn faithful_imported_memory_requires_sources_and_no_model_call() {
    let directory = directory();
    let database = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let (run, call) = fixture(&database).await;
    let mut tx = transaction(&database).await;
    let imported_observation = Observation {
        id: ObservationId::new(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        material_id: run.material_id.clone(),
        kind: ObservationKind::Import,
        observed_at: None,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
    };
    tx.insert_observation(&imported_observation).await.unwrap();
    let imported = Memory {
        origin: MemoryOrigin::Imported,
        model_call_id: None,
        prompt_version: None,
        observation_id: imported_observation.id.clone(),
        ..memory(&run, &call, tx.commit_seq())
    };
    tx.save_memory(&imported, None).await.unwrap();
    tx.append_memory_audit(&MemoryAudit {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        memory_id: imported.id.clone(),
        action: "import".into(),
        actor: "storage-test".into(),
        reason: "faithful source admission".into(),
        previous_revision: 0,
        revision: 1,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
        details: json!({"origin":"imported"}),
    })
    .await
    .unwrap();
    tx.commit(serde_json::to_value(&imported).unwrap())
        .await
        .unwrap();
    let stored = database
        .memory(&run.namespace_id, &imported.id)
        .await
        .unwrap();
    assert_eq!(stored.origin, MemoryOrigin::Imported);
    assert_eq!(
        database
            .observation(&run.namespace_id, &stored.observation_id)
            .await
            .unwrap()
            .kind,
        ObservationKind::Import
    );
    assert!(stored.model_call_id.is_none());
    assert!(stored.prompt_version.is_none());
    let creation = database
        .memory_audit(&run.namespace_id, &imported.id, 50)
        .await
        .unwrap();
    assert_eq!(creation.len(), 1);
    assert_eq!(
        (creation[0].previous_revision, creation[0].revision),
        (0, 1)
    );
    assert_eq!(creation[0].action, "import");
    assert!(database.model_calls(&run.job_id).await.unwrap().is_empty());
    assert_eq!(
        database.memory_changes(0, 1).await.unwrap().memories[0].id,
        imported.id
    );
    let mut tx = transaction(&database).await;
    let invalid = Memory {
        id: MemoryId::new(),
        origin: MemoryOrigin::Model,
        commit_seq: tx.commit_seq(),
        ..imported.clone()
    };
    assert_eq!(
        tx.save_memory(&invalid, None).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&database).await;
    let invalid = Memory {
        id: MemoryId::new(),
        model_call_id: Some(call.id),
        prompt_version: Some("fabricated".into()),
        commit_seq: tx.commit_seq(),
        ..imported.clone()
    };
    assert_eq!(
        tx.save_memory(&invalid, None).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&database).await;
    let trash = Memory {
        deleted: true,
        revision: 2,
        commit_seq: tx.commit_seq(),
        ..imported.clone()
    };
    tx.save_memory(&trash, Some(1)).await.unwrap();
    tx.commit(json!({"trashed":trash.id})).await.unwrap();
    let mut tx = transaction(&database).await;
    tx.purge_memory(&run.namespace_id, &imported.id, 2)
        .await
        .unwrap();
    tx.commit(json!({"purged":true})).await.unwrap();
    assert!(database.model_calls(&run.job_id).await.unwrap().is_empty());
    database.close().await;
}

#[tokio::test]
async fn maintenance_pages_pass_protected_prefix_and_hold_the_birth_boundary() {
    let directory = directory();
    let database = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let (run, call) = fixture(&database).await;
    let id = |suffix: &str| MemoryId(format!("00000000-0000-0000-0000-00000000000{suffix}"));
    let mut tx = transaction(&database).await;
    for suffix in ["1", "2", "3"] {
        let mut memory = Memory {
            id: id(suffix),
            origin: MemoryOrigin::Imported,
            model_call_id: None,
            prompt_version: None,
            ..memory(&run, &call, tx.commit_seq())
        };
        if suffix == "1" {
            memory.lifecycle.pinned = true;
        }
        if suffix == "2" {
            memory.lifecycle.activation = MemoryActivation::Frozen;
        }
        tx.save_memory(&memory, None).await.unwrap();
    }
    tx.commit(json!({"initial":3})).await.unwrap();
    let first = database
        .maintenance_page(&run.namespace_id, &run.subject_id, None, None, 2)
        .await
        .unwrap();
    assert_eq!(first.memories.len(), 2);
    assert!(first.has_more);
    assert!(first.memories[0].lifecycle.pinned);
    assert_eq!(
        first.memories[1].lifecycle.activation,
        MemoryActivation::Frozen
    );
    assert_eq!(first.next_cursor, Some(id("2")));
    let mut tx = transaction(&database).await;
    let mut updated = tx.memory(&run.namespace_id, &id("3")).await.unwrap();
    updated.revision = 2;
    updated.commit_seq = tx.commit_seq();
    updated.lifecycle.activation = MemoryActivation::Weakened;
    tx.save_memory(&updated, Some(1)).await.unwrap();
    let born_later = Memory {
        id: id("4"),
        origin: MemoryOrigin::Imported,
        model_call_id: None,
        prompt_version: None,
        ..memory(&run, &call, tx.commit_seq())
    };
    tx.save_memory(&born_later, None).await.unwrap();
    tx.commit(json!({"later":true})).await.unwrap();
    let next = database
        .maintenance_page(
            &run.namespace_id,
            &run.subject_id,
            first.next_cursor.as_ref(),
            Some(first.snapshot_seq),
            2,
        )
        .await
        .unwrap();
    assert_eq!(next.memories.len(), 1);
    assert_eq!(next.memories[0].id, id("3"));
    assert_eq!(
        next.memories[0].lifecycle.activation,
        MemoryActivation::Weakened
    );
    assert_eq!(next.memories[0].revision, 2);
    assert_eq!(next.snapshot_seq, first.snapshot_seq);
    assert!(!next.has_more);
    assert!(next.next_cursor.is_none());
    let fresh = database
        .maintenance_page(&run.namespace_id, &run.subject_id, None, None, 10)
        .await
        .unwrap();
    assert_eq!(fresh.memories.len(), 4);
    assert!(fresh.snapshot_seq > first.snapshot_seq);
    database.close().await;
}
