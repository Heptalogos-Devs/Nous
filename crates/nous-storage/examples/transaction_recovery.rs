//! Run against a fresh file under the project's temp directory:
//! cargo run -p nous-storage --example transaction_recovery -- <absolute-file>
//! This uses real worker child processes to test the commit/ACK crash gap.
//! Queue worker setup adapts apalis-sqlite examples/basic.rs; see LICENSE.apalis.
use apalis::prelude::*;
use apalis_sqlite::Config;
use nous_domain::*;
use nous_storage::*;
use serde_json::json;
use std::{path::Path, process::Command, time::Duration};

type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn key(id: &str, hash: &str) -> OperationKey {
    OperationKey {
        caller_id: "storage-spike".into(),
        operation: "namespace.create".into(),
        scope: "instance".into(),
        operation_id: id.into(),
        request_hash: hash.into(),
    }
}

async fn transaction(database: &Database, id: &str) -> TestResult<WriteTransaction> {
    let BeginOperation::New(tx) = database.begin_operation(key(id, "v1:request")).await? else {
        panic!("new operation expected")
    };
    Ok(tx)
}

async fn worker(path: &Path, crash: bool) -> TestResult<()> {
    let database = Database::open(path).await?;
    let backend = database
        .queue_backend()
        .with_config(
            Config::default()
                .queue(JOB_QUEUE)
                .heartbeat_interval(Duration::from_secs(1))
                .batch_size(1),
        )
        .poll_with_interval(Duration::from_millis(100));
    let worker = WorkerBuilder::new("nous-storage-recovery")
        .backend(backend)
        .data(database.clone())
        .build(
            move |job: JobEnvelope, database: Data<Database>, context: WorkerContext| async move {
                // Exactly the production adoption path: the queue may redeliver;
                // the completed authority operation returns its durable receipt.
                match database
                    .begin_operation(key(&job.id, "v1:adoption"))
                    .await?
                {
                    BeginOperation::New(tx) => {
                        tx.commit(json!({"adopted":job.id})).await?;
                    }
                    BeginOperation::Replay(receipt) => {
                        assert_eq!(receipt.result, json!({"adopted":job.id}));
                    }
                }
                if crash {
                    std::process::exit(73);
                }
                context.stop()?;
                Ok::<_, BoxDynError>(())
            },
        );
    tokio::time::timeout(Duration::from_secs(20), worker.run()).await??;
    database.close().await;
    Ok(())
}

#[tokio::main]
async fn main() -> TestResult<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if args[1] == "crash" || args[1] == "recover" {
        return worker(Path::new(&args[2]), args[1] == "crash").await;
    }
    let path = std::path::PathBuf::from(&args[1]);
    assert!(!path.exists(), "use a fresh database path");
    let database = Database::open(&path).await?;
    let ns_id = NamespaceId::new();
    let job_id = ulid::Ulid::generate().to_string();
    let mut tx = transaction(&database, "rollback").await?;
    let ns = Namespace {
        id: ns_id.clone(),
        name: "storage test".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    };
    tx.insert_namespace(&ns).await?;
    tx.enqueue(JobEnvelope {
        id: job_id.clone(),
        kind: "check".into(),
        namespace_id: ns_id.to_string(),
        subject_id: None,
        expected_revision: None,
        payload: json!({"schema":"spike.v1"}),
    })
    .await?;
    tx.rollback().await?;
    assert_eq!(database.status().await?.commit_seq, 0);
    assert_eq!(
        database.namespace(&ns_id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        database.job(&job_id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    let mut tx = transaction(&database, "commit").await?;
    let ns = Namespace {
        commit_seq: tx.commit_seq(),
        ..ns
    };
    tx.insert_namespace(&ns).await?;
    tx.enqueue(JobEnvelope {
        id: job_id.clone(),
        kind: "check".into(),
        namespace_id: ns_id.to_string(),
        subject_id: None,
        expected_revision: None,
        payload: json!({"schema":"spike.v1"}),
    })
    .await?;
    let receipt = tx.commit(serde_json::to_value(&ns)?).await?;
    assert_eq!(receipt.commit_seq, 1);
    assert_eq!(receipt.job_ids, vec![job_id.clone()]);
    database.close().await;
    let database = Database::open(&path).await?;
    assert_eq!(database.namespace(&ns_id).await?.name, "storage test");
    assert_eq!(database.job(&job_id).await?.status, "Pending");
    let BeginOperation::Replay(replayed) = database
        .begin_operation(key("commit", "v1:request"))
        .await?
    else {
        panic!("replay expected")
    };
    assert_eq!(receipt, replayed);
    assert_eq!(
        database
            .begin_operation(key("commit", "v1:changed"))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(database.status().await?.commit_seq, 1);

    let subject_id = SubjectId::new();
    let material_id = MaterialId::new();
    let mut tx = transaction(&database, "scope-fixture").await?;
    tx.insert_subject(&Subject {
        id: subject_id.clone(),
        namespace_id: ns_id.clone(),
        name: "subject".into(),
        capabilities: vec![Capability::Memory],
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await?;
    tx.insert_material(&Material {
        id: material_id.clone(),
        namespace_id: ns_id.clone(),
        content_hash: "example-digest".into(),
        media_type: "text/plain".into(),
        byte_len: 4,
        source: Some("fixture".into()),
        event_time: Some(TimePoint {
            domain: "world/1".into(),
            coordinate: 42,
        }),
        recorded_at: "2026-10-23T00:00:00Z".into(),
        commit_seq: tx.commit_seq(),
    })
    .await?;
    for _ in 0..2 {
        tx.insert_observation(&Observation {
            id: ObservationId::new(),
            namespace_id: ns_id.clone(),
            subject_id: subject_id.clone(),
            material_id: material_id.clone(),
            observed_at: None,
            recorded_at: "2026-10-23T00:00:00Z".into(),
            commit_seq: tx.commit_seq(),
        })
        .await?;
    }
    tx.commit(json!({"fixture":true})).await?;
    assert_eq!(database.list_observations(&subject_id).await?.len(), 2);
    assert!(
        database
            .material_is_observed(&ns_id, &subject_id, &material_id)
            .await?
    );
    assert_eq!(
        database
            .material(&NamespaceId::new(), &material_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let other = NamespaceId::new();
    let mut tx = transaction(&database, "other-namespace").await?;
    tx.insert_namespace(&Namespace {
        id: other.clone(),
        name: "other".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await?;
    tx.commit(json!({"fixture":true})).await?;
    let mut tx = transaction(&database, "cross-scope").await?;
    let error = tx
        .insert_observation(&Observation {
            id: ObservationId::new(),
            namespace_id: other,
            subject_id,
            material_id,
            observed_at: None,
            recorded_at: "2026-10-23T00:00:00Z".into(),
            commit_seq: tx.commit_seq(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidArgument);
    tx.rollback().await?;
    assert_eq!(database.status().await?.commit_seq, 3);

    // Concurrent retries serialize at BEGIN IMMEDIATE and return one receipt.
    let mut retries = Vec::new();
    for _ in 0..8 {
        let database = database.clone();
        retries.push(tokio::spawn(async move {
            match database
                .begin_operation(key("concurrent", "v1:request"))
                .await?
            {
                BeginOperation::Replay(receipt) => Ok::<_, Error>(receipt),
                BeginOperation::New(tx) => tx.commit(json!({"concurrent":true})).await,
            }
        }));
    }
    let mut receipts = Vec::new();
    for retry in retries {
        receipts.push(retry.await??);
    }
    assert!(receipts.iter().all(|r| r == &receipts[0]));
    assert_eq!(database.status().await?.commit_seq, 4);
    database.close().await;

    let executable = std::env::current_exe()?;
    assert_eq!(
        Command::new(&executable)
            .arg("crash")
            .arg(&path)
            .status()?
            .code(),
        Some(73)
    );
    let database = Database::open(&path).await?;
    assert_eq!(database.status().await?.commit_seq, 5);
    assert!(matches!(
        database.job(&job_id).await?.status.as_str(),
        "Running" | "Queued"
    ));
    database.close().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        Command::new(&executable)
            .arg("recover")
            .arg(&path)
            .status()?
            .success()
    );
    let database = Database::open(&path).await?;
    assert_eq!(database.job(&job_id).await?.status, "Done");
    assert!(database.job(&job_id).await?.attempts >= 1);
    assert_eq!(database.status().await?.commit_seq, 5);
    println!(
        "{}",
        json!({"storage_api":true,"rollback":true,"reopen":true,"idempotent_replay":true,"changed_payload_conflict":true,"concurrent_retries":8,"commit_sequence":5,"two_observations":true,"scope_foreign_keys":true,"worker_process_crash":true,"redelivery_adopted_once":true,"database":path})
    );
    database.close().await;
    Ok(())
}
