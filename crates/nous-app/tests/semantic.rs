//! Real SDK/HTTP, durable worker, SQL authority and persisted ANN/serving composition.
//! The local provider returns fixed numerical fixtures and never contacts a model service.
use axum::{Json, Router, routing::post};
use nous_app::model::embedding::EMBEDDING_PRODUCER_VERSION;
use nous_app::{
    App,
    instance::{InstanceConfig, InstancePaths, SecretRef},
    model::{EmbeddingConfig, ModelConfig, ModelProtocol, ModelResource},
};
use nous_domain::*;
use nous_storage::OperationReceipt;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const MODEL: &str = "semantic-fixture-3d";
const KEY_FILE: &str = "credentials/semantic-fixture.token";
const QUERY_ALPHA: &str = "silent astronomy";
const QUERY_BETA: &str = "pelagic coordinate";

struct Provider {
    endpoint: String,
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<Value>>>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
    task: JoinHandle<()>,
}
impl Provider {
    async fn start(block_marker: Option<&'static str>) -> Self {
        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let count = calls.clone();
        let log = requests.clone();
        let start = entered.clone();
        let finish = release.clone();
        let router=Router::new().route("/v1/embeddings",post(move |Json(body):Json<Value>|{
            let count=count.clone();let log=log.clone();let start=start.clone();let finish=finish.clone();
            async move {
                count.fetch_add(1,Ordering::SeqCst);
                assert_eq!(body["model"],"semantic-fixture-alias");
                log.lock().unwrap().push(body.clone());
                let inputs=match &body["input"] {
                    Value::String(text)=>vec![text.clone()],
                    Value::Array(inputs)=>inputs.iter().map(|input|input.as_str().expect("text embedding input").to_owned()).collect(),
                    other=>panic!("unexpected embedding input {other}"),
                };
                if block_marker.is_some_and(|marker|inputs.iter().any(|input|input.contains(marker))) {
                    start.notify_one();finish.notified().await;
                }
                let data=inputs.iter().enumerate().map(|(index,text)|{
                    let vector=if text.contains("CORAL") || text.contains("REEF") || text==QUERY_BETA {vec![0.0,1.0,0.0]}
                        else if text.contains("FUTURE") {vec![0.0,0.0,1.0]}
                        else if text.contains("COMMON") {vec![0.6,0.8,0.0]}
                        else {vec![1.0,0.0,0.0]};
                    json!({"object":"embedding","index":index,"embedding":vector})
                }).collect::<Vec<_>>();
                Json(json!({"object":"list","model":MODEL,"data":data,"usage":{"prompt_tokens":inputs.len()*7,"total_tokens":inputs.len()*7}}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            endpoint,
            calls,
            requests,
            entered,
            release,
            task,
        }
    }
    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}
fn instance(provider: &Provider) -> (tempfile::TempDir, InstancePaths) {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../temp/instances/semantic-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(directory.path()).unwrap();
    paths.initialize().unwrap();
    std::fs::write(
        paths.resolve(KEY_FILE).unwrap(),
        "local-test-credential-not-a-provider-secret-0002",
    )
    .unwrap();
    let mut config = InstanceConfig::load(&paths).unwrap();
    config.models = ModelConfig {
        memory_formation: None,
        text_embedding: Some(EmbeddingConfig {
            resource: ModelResource {
                id: "semantic-vectors".into(),
                provider: "test".into(),
                protocol: ModelProtocol::OpenAiEmbeddings,
                endpoint: provider.endpoint.clone(),
                model: "semantic-fixture-alias".into(),
                credential: SecretRef {
                    file: KEY_FILE.into(),
                },
            },
            expected_provider_model: MODEL.into(),
            dimension: 3,
            max_batch_size: 32,
            max_input_bytes: 32768,
            max_calls_per_day: 100,
            producer_version: EMBEDDING_PRODUCER_VERSION.into(),
        }),
    };
    std::fs::write(&paths.config, toml::to_string_pretty(&config).unwrap()).unwrap();
    (directory, paths)
}
async fn scope(app: &App) -> (Namespace, Subject, CallContext) {
    let admin = CallContext::admin();
    let ns = serde_json::from_value::<Namespace>(
        app.create_namespace(&admin, "semantic-ns", "semantic")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let subject = serde_json::from_value::<Subject>(
        app.create_subject(
            &admin,
            "semantic-reader",
            &ns.id,
            "reader",
            vec![Capability::Memory],
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let ctx = CallContext {
        caller_id: "semantic-reader-token".into(),
        scope: AccessScope::Subject {
            namespace_id: ns.id.clone(),
            subject_id: subject.id.clone(),
        },
    };
    (ns, subject, ctx)
}
fn query(subject: &SubjectId, text: &str) -> QueryRequest {
    QueryRequest {
        mode: QueryMode::Vcp,
        subject_id: subject.clone(),
        text: text.into(),
        reference: None,
        source: None,
        time: Some(TimeFilter {
            axis: TimeAxis::Event,
            range: TimeRange {
                domain: "fixture-clock".into(),
                start: Some(0),
                end: Some(100),
            },
        }),
        limit: 10,
        min_commit_seq: None,
        include_inactive: false,
    }
}
async fn import(
    app: &App,
    ctx: &CallContext,
    ns: &NamespaceId,
    subject: &SubjectId,
    rows: Vec<Value>,
) -> Vec<MemoryId> {
    let result = app
        .import_text(
            ctx,
            ImportRequest {
                namespace_id: ns.clone(),
                subject_id: subject.clone(),
                format: ImportFormat::Json,
                content: serde_json::to_string(&rows).unwrap(),
                form_memory: false,
                source: Some("test:semantic".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.accepted, rows.len(), "{result:#?}");
    result
        .items
        .iter()
        .map(|item| {
            serde_json::from_value(item.receipt.as_ref().unwrap()["result"]["memory_id"].clone())
                .unwrap()
        })
        .collect()
}
async fn wait_run(app: &App, ctx: &CallContext, job: &str, state: EmbeddingState) -> EmbeddingRun {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let run = app.get_embedding_run(ctx, job).await.unwrap();
            if run.state == state {
                return run;
            }
            assert!(
                !matches!(
                    run.state,
                    EmbeddingState::Failed | EmbeddingState::Uncertain | EmbeddingState::Cancelled
                ),
                "unexpected embedding state {run:#?}"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("embedding run reached its expected state")
}
async fn sync(
    app: &App,
    ctx: &CallContext,
    subject: &SubjectId,
    key: &str,
) -> (OperationReceipt, Vec<EmbeddingRun>) {
    let receipt = app.sync_embeddings(ctx, key, subject, 100).await.unwrap();
    let jobs = serde_json::from_value::<Vec<String>>(receipt.result["jobs"].clone()).unwrap();
    let mut runs = Vec::new();
    for job in jobs {
        runs.push(wait_run(app, ctx, &job, EmbeddingState::Succeeded).await);
    }
    (receipt, runs)
}
fn change(action: MemoryAction, revision: i64) -> MemoryChange {
    MemoryChange {
        action,
        expected_revision: revision,
        strength: None,
        hours: None,
        reason: "semantic fixture transition".into(),
        force: false,
        edit: None,
    }
}
async fn mutate(
    app: &App,
    ctx: &CallContext,
    id: &MemoryId,
    action: MemoryAction,
    key: &str,
) -> Memory {
    let previous = app.get_memory(ctx, id).await.unwrap();
    serde_json::from_value(
        app.change_memory(ctx, key, id, change(action, previous.revision))
            .await
            .unwrap()
            .result,
    )
    .unwrap()
}

#[tokio::test]
async fn adopted_embeddings_drive_vcp_source_filters_content_reuse_and_offline_recovery() {
    let provider = Provider::start(None).await;
    let (_directory, paths) = instance(&provider);
    let app = App::open(paths.clone()).await.unwrap();
    let (ns, subject, ctx) = scope(&app).await;
    let ids=import(&app,&ctx,&ns.id,&subject.id,vec![
        json!({"external_id":"alpha","text":"EMBER_ARCHIVE documents nocturnal owls","source":"test:alpha","tags":["EMBER_TAG","COMMON_TAG"],"event_time":{"domain":"fixture-clock","coordinate":10}}),
        json!({"external_id":"beta","text":"CORAL_ARCHIVE documents migrating whales","source":"test:beta","tags":["CORAL_TAG","COMMON_TAG"],"event_time":{"domain":"fixture-clock","coordinate":20}}),
        json!({"external_id":"future","text":"FUTURE_ARCHIVE reports another event","source":"test:alpha","tags":["FUTURE_TAG"],"event_time":{"domain":"fixture-clock","coordinate":1000}}),
    ]).await;
    let cancellation = CancellationToken::new();
    let worker = app.start_background(cancellation.clone());
    let (_, runs) = sync(&app, &ctx, &subject.id, "initial-vectors").await;
    assert_eq!(runs.len(), 1);
    assert_eq!(provider.count(), 1);
    let batch_call = runs[0].model_call_id.clone().unwrap();
    let call = app.read_model_call(&ctx, &batch_call).await.unwrap();
    assert_eq!(call.state, ModelCallState::Succeeded);
    assert_eq!(call.request["role"], "text_embedding");
    assert_eq!(call.request["space"]["dimension"], 3);
    assert_eq!(
        call.response.as_ref().unwrap()["record"]["provider_model"],
        MODEL
    );
    assert!(
        provider.requests.lock().unwrap()[0]["input"]
            .to_string()
            .contains("EMBER_ARCHIVE")
    );
    let original = app.memory_source(&ctx, &ids[0]).await.unwrap();
    let original_vector = original
        .embedding_provenance
        .iter()
        .find(|p| p.memory_revision == Some(original.memory.revision))
        .expect("adopted Memory vector provenance")
        .clone();
    assert_eq!(original_vector.model_call_id, batch_call);

    let mut lexical = query(&subject.id, QUERY_ALPHA);
    lexical.mode = QueryMode::Lexical;
    assert!(app.query(&ctx, lexical).await.unwrap().hits.is_empty());
    let first = app
        .query(&ctx, query(&subject.id, QUERY_ALPHA))
        .await
        .unwrap();
    assert_eq!(first.hits[0].memory.id, ids[0]);
    assert!(first.hits.iter().all(|hit| hit.memory.id != ids[2]));
    let semantic = first
        .semantic
        .as_ref()
        .expect("actual semantic serving diagnostics");
    assert_eq!(semantic.mode, QueryMode::Vcp);
    assert_eq!(semantic.eligible_memories, 2);
    assert_eq!(
        app.read_model_call(&ctx, &semantic.query_embedding_call_id)
            .await
            .unwrap()
            .state,
        ModelCallState::Succeeded
    );
    let after_query = provider.count();
    assert_eq!(after_query, 2);
    let repeated = app
        .query(&ctx, query(&subject.id, QUERY_ALPHA))
        .await
        .unwrap();
    assert_eq!(provider.count(), after_query);
    assert_eq!(repeated.semantic.unwrap().generation, semantic.generation);

    let frozen = mutate(&app, &ctx, &ids[1], MemoryAction::Freeze, "freeze-beta").await;
    let (reused, new_runs) = sync(&app, &ctx, &subject.id, "reuse-after-freeze").await;
    assert_eq!(reused.result["reused_vectors"], 1);
    assert!(new_runs.is_empty());
    assert_eq!(provider.count(), after_query);
    let beta = app.memory_source(&ctx, &ids[1]).await.unwrap();
    assert_eq!(
        beta.embedding_provenance
            .iter()
            .find(|p| p.memory_revision == Some(frozen.revision))
            .expect("lifecycle revision reuses accepted vector")
            .model_call_id,
        batch_call
    );
    let mut narrowed = query(&subject.id, QUERY_ALPHA);
    narrowed.source = Some("test:alpha".into());
    let filtered = app.query(&ctx, narrowed.clone()).await.unwrap();
    assert_eq!(
        filtered
            .hits
            .iter()
            .map(|hit| hit.memory.id.clone())
            .collect::<Vec<_>>(),
        vec![ids[0].clone()]
    );
    let diagnostics = &filtered.semantic.as_ref().unwrap().diagnostics["serving"];
    assert_eq!(filtered.semantic.as_ref().unwrap().eligible_memories, 1);
    assert_eq!(diagnostics["assets"]["source_root_count"], 1);
    assert_eq!(diagnostics["assets"]["tag_count"], 2);
    let outside = CallContext {
        caller_id: "another-subject".into(),
        scope: AccessScope::Subject {
            namespace_id: ns.id.clone(),
            subject_id: SubjectId::new(),
        },
    };
    assert_eq!(
        app.query(&outside, narrowed).await.unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(provider.count(), after_query);

    let current = app.get_memory(&ctx, &ids[0]).await.unwrap();
    let mut revision = change(MemoryAction::Revise, current.revision);
    revision.edit = Some(MemoryEdit {
        title: None,
        content: Some("REEF_REVISED describes a different population".into()),
        epistemic_status: None,
    });
    let revised: Memory = serde_json::from_value(
        app.change_memory(&ctx, "revise-content", &ids[0], revision)
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    assert!(
        !app.memory_source(&ctx, &ids[0])
            .await
            .unwrap()
            .embedding_provenance
            .iter()
            .any(|p| p.memory_revision == Some(revised.revision))
    );
    let (_, changed) = sync(&app, &ctx, &subject.id, "changed-content-vector").await;
    assert_eq!(changed.len(), 1);
    assert_eq!(provider.count(), after_query + 1);
    let revised_source = app.memory_source(&ctx, &ids[0]).await.unwrap();
    let new_vector = revised_source
        .embedding_provenance
        .iter()
        .find(|p| p.memory_revision == Some(revised.revision))
        .unwrap();
    assert_ne!(new_vector.model_call_id, original_vector.model_call_id);
    assert_ne!(new_vector.source_hash, original_vector.source_hash);
    let mut beta_query = query(&subject.id, QUERY_BETA);
    beta_query.source = Some("test:alpha".into());
    let updated = app.query(&ctx, beta_query.clone()).await.unwrap();
    assert_eq!(updated.hits[0].memory.revision, revised.revision);
    assert_eq!(provider.count(), after_query + 2);
    let mut hybrid_query = beta_query.clone();
    hybrid_query.mode = QueryMode::Hybrid;
    hybrid_query.text = "CORAL".into();
    let hybrid = app.query(&ctx, hybrid_query).await.unwrap();
    assert_eq!(hybrid.hits.len(), 1);
    assert_eq!(hybrid.hits[0].memory.id, ids[0]);
    assert_eq!(hybrid.hits[0].memory.revision, revised.revision);
    let cached_calls = provider.count();
    cancellation.cancel();
    worker.await.unwrap();
    app.close().await;
    std::fs::remove_file(paths.resolve(KEY_FILE).unwrap()).unwrap();
    provider.task.abort();
    let _ = provider.task.await;
    let reopened = App::open(paths).await.unwrap();
    let recovered = reopened.query(&ctx, beta_query.clone()).await.unwrap();
    assert_eq!(
        recovered.semantic.as_ref().unwrap().diagnostics["restored"],
        true
    );
    assert_eq!(recovered.hits[0].memory.id, ids[0]);
    assert_eq!(recovered.hits[0].memory.revision, revised.revision);
    assert_eq!(provider.calls.load(Ordering::SeqCst), cached_calls);
    assert_eq!(
        reopened
            .memory_source(&ctx, &ids[0])
            .await
            .unwrap()
            .embedding_provenance
            .iter()
            .find(|p| p.memory_revision == Some(revised.revision))
            .unwrap()
            .model_call_id,
        new_vector.model_call_id
    );
    mutate(
        &reopened,
        &ctx,
        &ids[0],
        MemoryAction::Trash,
        "trash-current",
    )
    .await;
    assert!(
        reopened
            .query(&ctx, beta_query)
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    reopened.close().await;
}

#[tokio::test]
async fn late_embedding_response_cannot_restore_a_permanently_purged_source() {
    const MARKER: &str = "PURGE_PRIVATE_EMBEDDING_BODY_91C";
    let provider = Provider::start(Some(MARKER)).await;
    let (_directory, paths) = instance(&provider);
    let app = App::open(paths.clone()).await.unwrap();
    let (ns, subject, ctx) = scope(&app).await;
    let ids=import(&app,&ctx,&ns.id,&subject.id,vec![json!({"external_id":"purge","text":MARKER,"tags":["ordinary"],"event_time":{"domain":"fixture-clock","coordinate":10}})]).await;
    let cancellation = CancellationToken::new();
    let worker = app.start_background(cancellation.clone());
    let receipt = app
        .sync_embeddings(&ctx, "in-flight-purge", &subject.id, 100)
        .await
        .unwrap();
    let job = receipt.result["jobs"][0].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(10), provider.entered.notified())
        .await
        .expect("real provider request entered");
    let running = app.get_embedding_run(&ctx, job).await.unwrap();
    assert_eq!(running.state, EmbeddingState::Running);
    let call_id = running.model_call_id.unwrap();
    assert_eq!(provider.count(), 1);
    let trashed = mutate(
        &app,
        &ctx,
        &ids[0],
        MemoryAction::Trash,
        "trash-before-response",
    )
    .await;
    let mut purge = change(MemoryAction::Purge, trashed.revision);
    purge.force = true;
    app.change_memory(
        &CallContext::admin(),
        "purge-before-response",
        &ids[0],
        purge,
    )
    .await
    .unwrap();
    let cancelled = app.get_embedding_run(&ctx, job).await.unwrap();
    assert_eq!(cancelled.state, EmbeddingState::Cancelled);
    assert!(
        cancelled
            .inputs
            .iter()
            .filter(|input| input.kind == EmbeddingKind::Memory)
            .all(|input| input.text.is_empty())
    );
    provider.release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let queued = app.job(&ctx, job).await.unwrap();
            let queue = queued.get("queue_status").unwrap_or(&queued["status"]);
            if queue == "Done" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("late response finished the real worker dispatch");
    let after = app.get_embedding_run(&ctx, job).await.unwrap();
    assert_eq!(after.state, EmbeddingState::Cancelled);
    assert!(!serde_json::to_string(&after).unwrap().contains(MARKER));
    let call = app.read_model_call(&ctx, &call_id).await.unwrap();
    assert_eq!(call.request["redacted"], true);
    assert_eq!(call.state, ModelCallState::Succeeded);
    assert_eq!(
        call.response.as_ref().unwrap()["record"]["provider_model"],
        MODEL
    );
    assert!(call.response.as_ref().unwrap()["record"]["usage"].is_object());
    assert!(!call.request.to_string().contains(MARKER));
    if let Some(response) = &call.response {
        assert!(!response.to_string().contains(MARKER));
        assert!(response["vectors"].as_array().is_none_or(Vec::is_empty));
    }
    assert_eq!(
        app.get_memory(&ctx, &ids[0]).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    let again = app
        .sync_embeddings(&ctx, "after-purge-sync", &subject.id, 100)
        .await
        .unwrap();
    assert_eq!(again.result["pending_inputs"], 0);
    assert!(again.result["jobs"].as_array().unwrap().is_empty());
    assert_eq!(provider.count(), 1);
    cancellation.cancel();
    worker.await.unwrap();
    app.close().await;
    let reopened = App::open(paths).await.unwrap();
    assert_eq!(
        reopened.get_embedding_run(&ctx, job).await.unwrap().state,
        EmbeddingState::Cancelled
    );
    assert!(
        !serde_json::to_string(&reopened.read_model_call(&ctx, &call_id).await.unwrap())
            .unwrap()
            .contains(MARKER)
    );
    reopened.close().await;
    provider.task.abort();
    let _ = provider.task.await;
}
