use axum::{Json, Router, routing::post};
use nous_app::{
    App,
    instance::{InstanceConfig, InstancePaths, SecretRef},
    model::{FORMATION_PROMPT_VERSION, FormationConfig, ModelConfig, ModelProtocol, ModelResource},
};
use nous_domain::*;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

async fn wait_for(app: &App, id: &str, state: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = app.job(&CallContext::admin(), id).await.unwrap();
            if job["status"] == state {
                return job;
            }
            assert_ne!(job["status"], "failed", "{job:#}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("formation reached its expected state")
}

fn query(subject_id: SubjectId, text: &str) -> QueryRequest {
    QueryRequest {
        subject_id,
        text: text.into(),
        reference: None,
        source: None,
        time: None,
        limit: 10,
        min_commit_seq: None,
        include_inactive: false,
    }
}

#[tokio::test]
async fn waiting_work_resumes_after_configuration_and_keeps_authority_through_query_and_delete() {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../temp/instances/formation-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(directory.path()).unwrap();
    paths.initialize().unwrap();
    let app = App::open(paths.clone()).await.unwrap();
    let admin = CallContext::admin();
    let ns: Namespace = serde_json::from_value(
        app.create_namespace(&admin, "ns", "formation")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let subject: Subject = serde_json::from_value(
        app.create_subject(
            &admin,
            "subject",
            &ns.id,
            "reader",
            vec![Capability::Memory],
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let material: Material = serde_json::from_value(
        app.put_material(
            &admin,
            "source",
            &ns.id,
            "京都研究组记录编号NW-M1-7421：秋天的枫叶呈红色。",
            Some("test:field-record".into()),
            None,
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let observation: Observation = serde_json::from_value(
        app.record_observation(&admin, "contact", &subject.id, &material.id, None)
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let submitted = app
        .form_memory(&admin, "form", &subject.id, &observation.id)
        .await
        .unwrap();
    let job_id = &submitted.job_ids[0];
    let cancel = CancellationToken::new();
    let background = app.start_background(cancel.clone());
    wait_for(&app, job_id, "waiting_for_configuration").await;
    cancel.cancel();
    background.await.unwrap();
    app.close().await;

    // A real HTTP boundary exercises the SDK request and strict JSON decoder.
    // The external provider is controlled here; the end-to-end gateway smoke
    // separately uses the operator's actual subscription.
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    let provider=Router::new().route("/v1/chat/completions",post(move |Json(request):Json<Value>| {
        let requests=requests.clone();async move {
            requests.fetch_add(1,Ordering::SeqCst);
            assert_eq!(request["model"],"test-memory-model");
            assert!(request["messages"].to_string().contains("NW-M1-7421"));
            Json(json!({"id":"chatcmpl-formation-test","object":"chat.completion","created":1792710000,"model":"test-memory-model",
                "choices":[{"index":0,"message":{"role":"assistant","content":json!({"memories":[{"title":"京都枫叶记录","content":"京都研究组记录编号NW-M1-7421：秋天的枫叶呈红色。","epistemic_status":"reported"}]}).to_string()},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":120,"completion_tokens":50,"total_tokens":170}}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
    let provider_task = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    let token_path = paths.resolve("credentials/test-model.token").unwrap();
    std::fs::write(
        token_path,
        "local-test-credential-not-a-provider-secret-0001",
    )
    .unwrap();
    let mut config = InstanceConfig::load(&paths).unwrap();
    config.models = ModelConfig {
        memory_formation: Some(FormationConfig {
            resource: ModelResource {
                id: "test-model".into(),
                provider: "test".into(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                endpoint,
                model: "test-memory-model".into(),
                credential: SecretRef {
                    file: "credentials/test-model.token".into(),
                },
            },
            prompt_version: FORMATION_PROMPT_VERSION.into(),
            max_input_bytes: 32768,
            max_output_tokens: 1000,
            max_calls_per_day: 1,
            native_parameters: Default::default(),
        }),
    };
    std::fs::write(&paths.config, toml::to_string_pretty(&config).unwrap()).unwrap();
    let reopened = App::open(paths.clone()).await.unwrap();
    let cancel = CancellationToken::new();
    let background = reopened.start_background(cancel.clone());
    let completed = wait_for(&reopened, job_id, "succeeded").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        completed["formation"]["memory_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let id: MemoryId =
        serde_json::from_value(completed["formation"]["memory_ids"][0].clone()).unwrap();
    let memory = reopened.get_memory(&admin, &id).await.unwrap();
    let source = reopened.memory_source(&admin, &id).await.unwrap();
    assert_eq!(
        source.material.text,
        "京都研究组记录编号NW-M1-7421：秋天的枫叶呈红色。"
    );
    assert_eq!(source.observation.id, observation.id);
    assert_eq!(
        source
            .model_call
            .as_ref()
            .unwrap()
            .response
            .as_ref()
            .unwrap()["record"]["usage"]["total_tokens"],
        170
    );
    assert!(
        source
            .model_call
            .as_ref()
            .unwrap()
            .cost_microunits
            .is_none()
    );
    let hits = reopened
        .query(&admin, query(subject.id.clone(), "京都 枫叶"))
        .await
        .unwrap();
    assert!(hits.hits.iter().any(|hit| hit.memory.id == id));
    assert_eq!(
        reopened
            .form_memory(&admin, "form", &subject.id, &observation.id)
            .await
            .unwrap(),
        submitted
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let extra = reopened
        .form_memory(&admin, "form-again", &subject.id, &observation.id)
        .await
        .unwrap();
    wait_for(&reopened, &extra.job_ids[0], "waiting_for_budget").await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    reopened
        .delete_memory(&admin, "remove", &id, memory.revision)
        .await
        .unwrap();
    assert!(
        reopened
            .query(&admin, query(subject.id.clone(), "京都 枫叶"))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    assert_eq!(reopened.memory_history(&admin, &id).await.unwrap().len(), 2);
    cancel.cancel();
    background.await.unwrap();
    reopened.close().await;
    provider_task.abort();
    let _ = provider_task.await;
    let final_app = App::open(paths).await.unwrap();
    assert!(
        final_app
            .query(&admin, query(subject.id, "京都 枫叶"))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    assert_eq!(
        final_app.memory_history(&admin, &id).await.unwrap().len(),
        2
    );
    final_app.close().await;
}
