// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_kernel::NousRuntime;
use test_support::{database, open_runtime};

#[tokio::test]
async fn only_one_kernel_runtime_owns_an_authority_database() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let root_conflict = nous_retrieval::ServingService::new(
        runtime.material.clone(),
        runtime.memory.clone(),
        runtime.serving.options.clone(),
        None,
        runtime.configuration.clone(),
    );
    let Err(error) = root_conflict else {
        panic!("a second ServingService opened the already-owned root");
    };
    assert!(format!("{error:?}").contains("InstanceAlreadyRunning"));

    let second = NousRuntime::open(nous_kernel::RuntimeOptions {
        postgres_url: url.clone(),
        max_connections: 4,
        acquire_timeout_ms: 15_000,
        object_root: root.path().join("objects").to_string_lossy().into_owned(),
        serving_options: nous_retrieval::ServingOptions {
            root: root.path().join("serving"),
            lexical: false,
            dense: false,
            topology: false,
            memory_enabled: true,
        },
        embedding: None,
        stored_embedding: None,
        core_descriptors: vec![],
        deployment_document: serde_json::json!({
            "capabilities": {
                "process": { "memory": true },
                "subject_defaults": { "memory": true }
            },
            "serving": {
                "lexical": { "enabled": false },
                "dense": { "enabled": false },
                "topology": { "enabled": false }
            }
        }),
    })
    .await;
    let Err(error) = second else {
        panic!("a second Kernel opened the already-owned Authority database");
    };
    assert!(format!("{error:?}").contains("InstanceAlreadyRunning"));

    runtime.store.close().await;
    drop(runtime);
    let reopened = open_runtime(&url, &root).await;
    reopened.store.close().await;
}
