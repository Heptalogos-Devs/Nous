//! Real apalis delivery across a child-process commit/ACK crash.
//! Use a new absolute database file under project temp.
use apalis::prelude::*;
use apalis_sqlite::Config;
use nous_domain::*;
use nous_storage::*;
use serde_json::json;
use std::{path::Path, process::Command, time::Duration};
type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn key(id: &str) -> OperationKey {
    OperationKey {
        caller_id: "recovery-example".into(),
        operation: "adopt".into(),
        scope: "instance".into(),
        operation_id: id.into(),
        request_hash: "adopt".into(),
    }
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
    let worker = WorkerBuilder::new(format!("recovery-{}", ulid::Ulid::generate()))
        .backend(backend)
        .data(database.clone())
        .build(
            move |job: JobEnvelope, database: Data<Database>, context: WorkerContext| async move {
                match database.begin_operation(key(&job.id)).await? {
                    BeginOperation::New(mut tx) => {
                        let mut task = tx.job_record(&job.id).await?;
                        task.state = JobState::Succeeded;
                        task.result = Some(json!({"adopted":job.id}));
                        tx.update_job_record(&task).await?;
                        tx.commit(json!({"adopted":job.id})).await?;
                    }
                    BeginOperation::Replay(receipt) => {
                        assert_eq!(receipt.result, json!({"adopted":job.id}))
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
    if matches!(args.get(1).map(String::as_str), Some("crash" | "recover")) {
        return worker(Path::new(&args[2]), args[1] == "crash").await;
    }
    let path = std::path::PathBuf::from(args.get(1).ok_or("provide a new absolute database path")?);
    assert!(path.is_absolute() && !path.exists());
    let database = Database::open(&path).await?;
    let task = JobRecord {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: None,
        subject_id: None,
        kind: "recovery.probe".into(),
        state: JobState::Queued,
        wait: None,
        checkpoint: None,
        result: None,
        error: None,
        created_at: "2026-10-23T00:00:00Z".into(),
        updated_at: "2026-10-23T00:00:00Z".into(),
    };
    let BeginOperation::New(mut tx) = database.begin_operation(key("submit")).await? else {
        unreachable!()
    };
    tx.insert_job_record(&task).await?;
    tx.commit(json!({"job_id":task.id})).await?;
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
    assert_eq!(
        database.job_record(&task.id).await?.state,
        JobState::Succeeded
    );
    assert!(matches!(
        database.queue_record(&task.id).await?.status.as_str(),
        "Running" | "Queued"
    ));
    assert_eq!(database.recover_interrupted_jobs().await?, 1);
    let saved_sequence = database.status().await?.commit_seq;
    database.close().await;
    assert!(
        Command::new(&executable)
            .arg("recover")
            .arg(&path)
            .status()?
            .success()
    );
    let database = Database::open(&path).await?;
    assert_eq!(database.queue_record(&task.id).await?.status, "Done");
    assert_eq!(database.status().await?.commit_seq, saved_sequence);
    assert_eq!(
        database.job_record(&task.id).await?.result,
        Some(json!({"adopted":task.id}))
    );
    database.close().await;
    println!(
        "{}",
        json!({"worker_crash":true,"redelivery_adopted_once":true,"database":path})
    );
    Ok(())
}
