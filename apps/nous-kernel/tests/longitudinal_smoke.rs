// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, TimeZone, Utc};
use nous_core::SubjectId;
use nous_kernel::{NousRuntime, RuntimeOptions};
use nous_retrieval::ServingOptions;
use nous_runtime::ManualCognitiveClock;
use serde::Deserialize;
use std::{path::Path, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
};
use tokio_stream::wrappers::TcpListenerStream;

struct Server {
    runtime: NousRuntime,
    endpoint: String,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<std::result::Result<(), tonic::transport::Error>>,
}
impl Server {
    async fn open(
        url: &str,
        root: &Path,
        clock: Arc<ManualCognitiveClock>,
        token: &str,
        configuration: &nous_configuration::ConfigurationBootstrapBundle,
    ) -> Self {
        let runtime = NousRuntime::open_with_clock(
            RuntimeOptions {
                postgres_url: url.into(),
                max_connections: 8,
                acquire_timeout_ms: 15000,
                object_root: root.join("objects").to_string_lossy().into_owned(),
                serving_options: ServingOptions {
                    root: root.join("serving"),
                    lexical: true,
                    dense: false,
                    topology: false,
                    memory_enabled: true,
                },
                embedding: None,
                stored_embedding: None,
                core_descriptors: configuration.core_descriptors.clone(),
                deployment_document: configuration.deployment_document.clone(),
            },
            clock,
        )
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(
            nous_kernel::transport::router(runtime.clone(), token.to_owned())
                .await
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = stopped.await;
                }),
        );
        Self {
            runtime,
            endpoint,
            stop,
            task,
        }
    }
    async fn close(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(std::time::Duration::from_secs(10), self.task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        self.runtime.store.close().await;
    }
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Control {
    Advance { subject: SubjectId, seconds: i64 },
    Restart,
    Done,
}

#[tokio::test]
async fn public_longitudinal_smoke() {
    tokio::time::timeout(std::time::Duration::from_secs(120), run_smoke())
        .await
        .expect("longitudinal smoke deadline");
}
async fn run_smoke() {
    let (root, url, _postgres) = test_support::database().await;
    let clock = Arc::new(ManualCognitiveClock::new(
        Utc.with_ymd_and_hms(2026, 10, 3, 0, 0, 0).unwrap(),
    ));
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // This public Core host uses the same owning startup bundle as ordinary serve.
    let bundle = tokio::process::Command::new(
        std::env::var_os("NOUS_LONGITUDINAL_NODE").unwrap_or_else(|| "node".into()),
    )
    .args(["--import", "tsx", "--input-type=module", "--eval",
        "import {configurationBundle} from './apps/nous-core/src/configuration/catalog.ts'; console.log(JSON.stringify(configurationBundle({serving:{lexical:{enabled:true},dense:{enabled:false},topology:{enabled:false}}})));",
    ])
    .current_dir(&repo)
    .kill_on_drop(true)
    .output().await.expect("Core configuration owner handoff");
    assert!(
        bundle.status.success(),
        "Core configuration export failed: {}",
        String::from_utf8_lossy(&bundle.stderr)
    );
    let configuration: nous_configuration::ConfigurationBootstrapBundle =
        serde_json::from_slice(&bundle.stdout).unwrap();
    let mut server = Server::open(&url, root.path(), clock.clone(), &token, &configuration).await;
    let mut child = tokio::process::Command::new(
        std::env::var_os("NOUS_LONGITUDINAL_NODE").unwrap_or_else(|| "node".into()),
    )
    .arg(repo.join("node_modules/tsx/dist/cli.mjs"))
    .arg(repo.join("scripts/smoke/longitudinal.ts"))
    .current_dir(&repo)
    .env("NOUS_LONGITUDINAL_ENDPOINT", &server.endpoint)
    .env("NOUS_LONGITUDINAL_TOKEN", &token)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .kill_on_drop(true)
    .spawn()
    .expect("launch Core smoke");
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut input = child.stdin.take().unwrap();
    while let Some(line) = output.next_line().await.unwrap() {
        let command: Control = serde_json::from_str(&line).expect("smoke control record");
        let response = match command {
            Control::Advance { subject, seconds } => {
                clock
                    .advance_by(subject, Duration::seconds(seconds))
                    .unwrap();
                serde_json::json!({"advanced":true})
            }
            Control::Restart => {
                server.close().await;
                server =
                    Server::open(&url, root.path(), clock.clone(), &token, &configuration).await;
                serde_json::json!({"endpoint":server.endpoint})
            }
            Control::Done => break,
        };
        input
            .write_all(format!("{response}\n").as_bytes())
            .await
            .unwrap();
        input.flush().await.unwrap();
    }
    drop(input);
    server.close().await;
    assert!(
        child.wait().await.unwrap().success(),
        "public longitudinal smoke failed"
    );
}
