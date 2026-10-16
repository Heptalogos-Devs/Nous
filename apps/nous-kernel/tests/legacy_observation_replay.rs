// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, process::Stdio};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
const TABLES: &[&str] = &[
    "subjects",
    "subject_capabilities",
    "artifacts",
    "observation_occurrences",
    "source_regions",
    "observation_request_bindings",
    "model_results",
    "self_objects",
    "self_revisions",
    "self_evidence",
    "self_dependencies",
    "mutation_receipts",
];
async fn sdk(stage: &str, project: &Path, endpoint: &str, token: &str, file: &Path) {
    let (stage, script) = stage.strip_suffix("-self").map_or(
        (stage, "packages/client/tests/legacy-observation-replay.ts"),
        |stage| (stage, "packages/client/tests/legacy-self-purpose-replay.ts"),
    );
    let result = tokio::process::Command::new("node")
        .args(["--import", "tsx", script, stage])
        .arg(project)
        .arg(endpoint)
        .arg(token)
        .arg(file)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{} {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one actual legacy process or frozen legacy database journey proves migration, canonical SDK replay, digest and retained result identity"
)]
async fn migrated_legacy_inline_and_artifact_requests_replay_without_new_occurrences() {
    let (root, url, _postgres) = test_support::database().await;
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture_file = project.join("apps/nous-kernel/tests/fixtures/legacy-world-purpose.json");
    let file = root.path().join("requests.json");
    let self_file = root.path().join("self-requests.json");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    if let Some(binary) = std::env::var_os("NOUS_TIME_LEGACY_KERNEL") {
        let old_project = std::env::var_os("NOUS_TIME_LEGACY_PROJECT")
            .expect("old official SDK project is required");
        let config = root.path().join("old-kernel.json");
        tokio::fs::write(&config,serde_json::to_vec(&serde_json::json!({"bootstrap":{"database":{"mode":"external","url":url,"max_connections":1,"acquire_timeout_ms":3000,"name":"","install_dir":"","data_dir":"","instance_dir":"","secret_dir":""},"object_root":root.path().join("objects"),"serving_root":root.path().join("serving")},"configuration":{"core_descriptors":[],"deployment_document":{"serving":{"lexical":{"enabled":false},"dense":{"enabled":false},"topology":{"enabled":false}}}}})).unwrap()).await.unwrap();
        let mut child = tokio::process::Command::new(binary)
            .arg("--config")
            .arg(config)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let token = "a".repeat(64);
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(format!("{token}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            BufReader::new(child.stdout.take().unwrap())
                .lines()
                .next_line(),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        let endpoint: Value = serde_json::from_str(&line).unwrap();
        sdk(
            "old",
            Path::new(&old_project),
            endpoint["endpoint"].as_str().unwrap(),
            &token,
            &file,
        )
        .await;
        sdk(
            "old-self",
            Path::new(&old_project),
            endpoint["endpoint"].as_str().unwrap(),
            &token,
            &self_file,
        )
        .await;
        drop(stdin);
        assert!(child.wait().await.unwrap().success());
        let request: Value =
            serde_json::from_slice(&tokio::fs::read(&file).await.unwrap()).unwrap();
        let mut tables = BTreeMap::<String, Vec<Value>>::new();
        for table in TABLES {
            let sql = format!("SELECT to_jsonb(r) FROM {table} r");
            tables.insert(
                (*table).into(),
                sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
                    .fetch_all(&pool)
                    .await
                    .unwrap(),
            );
        }
        let self_request: Value =
            serde_json::from_slice(&tokio::fs::read(&self_file).await.unwrap()).unwrap();
        let fixture = serde_json::json!({"self_request":self_request,"source":"actual official 5bea000e kernel / SDK; isolated PG max_connections=1","request":request,"tables":tables});
        if std::env::var_os("NOUS_TIME_CAPTURE_LEGACY_FIXTURE").is_some() {
            tokio::fs::write(&fixture_file, serde_json::to_vec_pretty(&fixture).unwrap())
                .await
                .unwrap();
        }
    } else {
        sqlx::migrate!("../../crates/persistence/migrations")
            .run_to(13, &pool)
            .await
            .unwrap();
        let fixture: Value = serde_json::from_slice(
            &tokio::fs::read(fixture_file)
                .await
                .expect("captured actual legacy fixture"),
        )
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET CONSTRAINTS ALL DEFERRED")
            .execute(&mut *tx)
            .await
            .unwrap();
        for table in TABLES {
            let sql = format!(
                "INSERT INTO {table} SELECT * FROM jsonb_populate_record(NULL::{table},$1)"
            );
            for row in fixture["tables"][table].as_array().unwrap() {
                sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                    .bind(row)
                    .execute(&mut *tx)
                    .await
                    .unwrap();
            }
        }
        tx.commit().await.unwrap();
        tokio::fs::write(
            &self_file,
            serde_json::to_vec(&fixture["self_request"]).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(&file, serde_json::to_vec(&fixture["request"]).unwrap())
            .await
            .unwrap();
    }
    let legacy_version: i64 = sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(legacy_version, 13);
    let before: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT request_id,request_digest FROM observation_request_bindings ORDER BY request_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    pool.close().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let token = "legacy-replay-current-fixture";
    let server = tokio::spawn(
        nous_kernel::transport::router(runtime.clone(), token.into())
            .await
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    sdk("new", &project, &endpoint, token, &file).await;
    sdk("new-self", &project, &endpoint, token, &self_file).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM observation_occurrences")
        .fetch_one(runtime.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 3);
    let after: Vec<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT request_id,request_digest FROM observation_request_bindings ORDER BY request_id",
    )
    .fetch_all(runtime.store.pool())
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "the migration retains legacy semantic request bindings"
    );
    let results: i64 = sqlx::query_scalar("SELECT count(*) FROM model_results")
        .fetch_one(runtime.store.pool())
        .await
        .unwrap();
    assert_eq!(
        results, 2,
        "replay does not regenerate a saved model result"
    );

    server.abort();
    runtime.store.close().await;
}
