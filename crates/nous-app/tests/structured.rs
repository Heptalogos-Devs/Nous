use axum::{Json, Router, routing::post};
use nous_app::{
    App,
    instance::{InstanceConfig, InstancePaths},
    structured::{FormStructuredRequest, FormationTarget, StructuredQuery},
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

async fn setup() -> (
    tempfile::TempDir,
    InstancePaths,
    App,
    Namespace,
    Subject,
    Vec<Observation>,
) {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../temp/instances/structured-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(directory.path()).unwrap();
    paths.initialize().unwrap();
    let app = App::open(paths.clone()).await.unwrap();
    let ctx = CallContext::admin();
    let ns = serde_json::from_value::<Namespace>(
        app.create_namespace(&ctx, "ns", "structured")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let subject = serde_json::from_value::<Subject>(
        app.create_subject(&ctx, "subject", &ns.id, "reader", vec![Capability::Memory])
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let mut observations = Vec::new();
    for (i, text) in [
        "Observatory NW-S-314 recorded a red maple in Kyoto.",
        "Independent botanical record NW-S-315: red maple after cold nights.",
    ]
    .iter()
    .enumerate()
    {
        let material: Material = serde_json::from_value(
            app.put_material(
                &ctx,
                &format!("source-{i}"),
                &ns.id,
                text,
                Some(format!("test:record-{i}")),
                None,
            )
            .await
            .unwrap()
            .result,
        )
        .unwrap();
        observations.push(
            serde_json::from_value(
                app.record_observation(
                    &ctx,
                    &format!("contact-{i}"),
                    &subject.id,
                    &material.id,
                    Some(TimePoint {
                        domain: "unix_ms".into(),
                        coordinate: 100 + i as i64,
                    }),
                )
                .await
                .unwrap()
                .result,
            )
            .unwrap(),
        );
    }
    (directory, paths, app, ns, subject, observations)
}
fn times() -> CognitiveTimes {
    let point = TimePoint {
        domain: "unix_ms".into(),
        coordinate: 100,
    };
    CognitiveTimes {
        event: None,
        observation: None,
        formation: point.clone(),
        recorded: point,
        validity: None,
    }
}
fn evidence(observation: &Observation) -> Vec<EvidenceLink> {
    vec![EvidenceLink {
        source: EvidenceSource::Observation {
            observation_id: observation.id.clone(),
        },
        polarity: EvidencePolarity::Support,
        relation: None,
        explanation: "Field record".into(),
    }]
}
fn concept(label: &str) -> CognitiveContent {
    CognitiveContent::Concept(ConceptContent {
        label: label.into(),
        description: "A red maple botanical concept".into(),
        aliases: vec![],
        canonical: None,
        members: vec![],
        lineage: vec![],
    })
}
fn request(
    ns: &Namespace,
    subject: &Subject,
    body: CognitiveContent,
    observation: &Observation,
) -> CreateStructuredRequest {
    CreateStructuredRequest {
        namespace_id: ns.id.clone(),
        subject_id: subject.id.clone(),
        expected_revision: 0,
        body,
        evidence: evidence(observation),
        times: times(),
        producer_call_id: None,
    }
}
fn query(subject: &Subject, text: &str) -> StructuredQuery {
    StructuredQuery {
        subject_id: subject.id.clone(),
        kind: None,
        text: text.into(),
        as_of_commit: None,
        include_deleted: false,
        time: None,
        limit: 20,
    }
}

#[tokio::test]
async fn structured_history_queries_and_sources_survive_revisions_and_restart() {
    let (_directory, paths, app, ns, subject, observations) = setup().await;
    let ctx = CallContext::admin();
    let input = request(&ns, &subject, concept("Maple tree"), &observations[0]);
    let receipt = app
        .create_structured(&ctx, "create", input.clone())
        .await
        .unwrap();
    assert_eq!(
        receipt,
        app.create_structured(&ctx, "create", input).await.unwrap()
    );
    let first: StructuredObject = serde_json::from_value(receipt.result).unwrap();
    let mut next = first.body.clone();
    if let CognitiveContent::Concept(ref mut value) = next {
        value.aliases.push("Acer".into());
        value.description = "Distinct scarlet foliage".into();
    }
    let revision = ReviseStructuredRequest {
        id: first.id.clone(),
        expected_revision: 1,
        body: next,
        evidence: first.evidence.clone(),
        times: first.times.clone(),
        producer_call_id: None,
    };
    app.revise_structured(&ctx, "revise", revision.clone())
        .await
        .unwrap();
    assert_eq!(
        app.revise_structured(&ctx, "stale", revision)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let source = app.structured_source(&ctx, &first.id, None).await.unwrap();
    assert_eq!(source["closure"]["roots"].as_array().unwrap().len(), 1);
    assert!(
        source["materials"][0]["text"]
            .as_str()
            .unwrap()
            .contains("NW-S-314")
    );
    assert_eq!(
        app.concept_by_alias(&ctx, &subject.id, " ACER ")
            .await
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        app.query_structured(&ctx, query(&subject, "scarlet"))
            .await
            .unwrap()["hits"][0]["object"]["revision"],
        2
    );
    let mut historic = query(&subject, "botanical");
    historic.as_of_commit = Some(first.commit_seq);
    assert_eq!(
        app.query_structured(&ctx, historic.clone()).await.unwrap()["hits"][0]["object"]["revision"],
        1
    );
    app.set_structured_deleted(&ctx, "retire", &first.id, 2, true)
        .await
        .unwrap();
    assert_eq!(
        app.query_structured(&ctx, query(&subject, "scarlet"))
            .await
            .unwrap()["hits"],
        json!([])
    );
    let mut deleted = query(&subject, "scarlet");
    deleted.include_deleted = true;
    assert_eq!(
        app.query_structured(&ctx, deleted).await.unwrap()["hits"][0]["object"]["deleted"],
        true
    );
    app.close().await;
    let app = App::open(paths).await.unwrap();
    assert_eq!(
        app.structured_history(&ctx, &first.id).await.unwrap().len(),
        3
    );
    assert_eq!(
        app.query_structured(&ctx, historic).await.unwrap()["hits"][0]["object"]["revision"],
        1
    );
    app.set_structured_deleted(&ctx, "restore", &first.id, 3, false)
        .await
        .unwrap();
    assert_eq!(
        app.concept_by_alias(&ctx, &subject.id, "acer")
            .await
            .unwrap()
            .revision,
        4
    );
    let outside = CallContext {
        caller_id: "other".into(),
        scope: AccessScope::Subject {
            namespace_id: ns.id,
            subject_id: SubjectId::new(),
        },
    };
    assert_eq!(
        app.structured_source(&outside, &first.id, None)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    app.close().await;
}

async fn wait(app: &App, id: &str, state: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let value = app.job(&CallContext::admin(), id).await.unwrap();
        if value["status"] == state {
            return value;
        }
        assert_ne!(value["status"], "failed", "{value:#}");
        assert!(
            tokio::time::Instant::now() < deadline,
            "expected {state}, got {value:#}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
fn raw_evidence(source: &str) -> Value {
    json!([{"source":source,"polarity":"support","relation":null,"explanation":"Field record"}])
}
fn raw_concept(slot: &str, label: &str) -> Value {
    json!({"slot_key":slot,"body":{"kind":"concept","content":{"label":label,"description":"Maple NW-S-314 concept","aliases":[],"canonical":null,"members":[],"lineage":[]}},"evidence":raw_evidence("s0")})
}

#[tokio::test]
async fn formation_waits_then_applies_independent_groups_with_shared_call_and_restart() {
    let (_directory, paths, app, ns, subject, observations) = setup().await;
    let ctx = CallContext::admin();
    let episode = EpisodeContent {
        title: "Recorded visit".into(),
        track: "field".into(),
        experience: TimeRange {
            domain: "unix_ms".into(),
            start: Some(90),
            end: Some(110),
        },
        parent: None,
        members: vec![EpisodeMember {
            source: EvidenceSource::Observation {
                observation_id: observations[0].id.clone(),
            },
            role: "event".into(),
            ordinal: 0,
        }],
        boundary_explanation: "One visit".into(),
        lineage: vec![],
    };
    let episode: StructuredObject = serde_json::from_value(
        app.create_structured(
            &ctx,
            "episode",
            request(
                &ns,
                &subject,
                CognitiveContent::Episode(episode),
                &observations[0],
            ),
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let targets = [
        ("maple", CognitiveKind::Concept),
        ("rollback", CognitiveKind::Concept),
        ("collision", CognitiveKind::Concept),
        ("dependent", CognitiveKind::Concept),
        ("journal", CognitiveKind::Journal),
        ("schema", CognitiveKind::Schema),
        ("association", CognitiveKind::Association),
    ];
    let request = FormStructuredRequest {
        subject_id: subject.id.clone(),
        sources: vec![
            EvidenceSource::Observation {
                observation_id: observations[0].id.clone(),
            },
            EvidenceSource::Observation {
                observation_id: observations[1].id.clone(),
            },
            EvidenceSource::CognitiveRevision {
                reference: episode.reference(),
            },
        ],
        targets: targets
            .iter()
            .map(|(key, kind)| FormationTarget {
                key: (*key).into(),
                kind: *kind,
                existing: None,
            })
            .collect(),
    };
    let receipt = app
        .form_structured(&ctx, "form", request.clone())
        .await
        .unwrap();
    assert_eq!(
        receipt,
        app.form_structured(&ctx, "form", request).await.unwrap()
    );
    let job_id = receipt.job_ids[0].clone();
    let cancel = CancellationToken::new();
    let worker = app.start_background(cancel.clone());
    wait(&app, &job_id, "waiting_for_configuration").await;
    cancel.cancel();
    worker.await.unwrap();
    app.close().await;
    let output = json!({"groups":[
        {"key":"concept","depends_on":[],"changes":[raw_concept("maple","Maple")]},
        {"key":"conflict","depends_on":[],"changes":[raw_concept("rollback","Must rollback"),raw_concept("collision","Maple")]},
        {"key":"dependent","depends_on":["conflict"],"changes":[raw_concept("dependent","Must skip")]},
        {"key":"journal","depends_on":[],"changes":[{"slot_key":"journal","body":{"kind":"journal","content":{"title":"Field journal","narrative":"The field visit recorded red maple.","period":{"domain":"unix_ms","start":90,"end":110},"perspective":{"class":"reported","attribution":"Field record","confidence":0.9},"points":[{"role":"summary","text":"Red maple was recorded.","evidence":raw_evidence("s0")}],"episodes":["s2"],"lineage":[]}},"evidence":raw_evidence("s0")}]},
        {"key":"schema","depends_on":[],"changes":[{"slot_key":"schema","body":{"kind":"schema","content":{"title":"Maple pattern","claim":"Two botanical records describe red maple.","scope":{"description":"These two records","about":[],"concepts":[],"validity":null},"boundary":"No global biological rule inferred","formation_mode":"synthesized","epistemic":{"class":"derived","attribution":"Two records","confidence":0.8},"support":["s0","s1"],"counterexamples":[],"boundary_cases":[],"lineage":[]}},"evidence":raw_evidence("s0")}]},
        {"key":"association","depends_on":["concept"],"changes":[{"slot_key":"association","body":{"kind":"association","content":{"from":"maple","to":"s2","relation":"documented_in","polarity":"positive","basis_class":"source_evidence","description":"Maple documented in the field episode"}},"evidence":raw_evidence("s0")}]}
    ]});
    let calls = Arc::new(AtomicUsize::new(0));
    let requests = calls.clone();
    let provider=Router::new().route("/v1/chat/completions",post(move |Json(request):Json<Value>|{let output=output.clone();let requests=requests.clone();async move{
        requests.fetch_add(1,Ordering::SeqCst);assert!(request["messages"].to_string().contains("NW-S-314"));
        Json(json!({"id":"structured-test","object":"chat.completion","created":1792710000,"model":"test-structured","choices":[{"index":0,"message":{"role":"assistant","content":output.to_string()},"finish_reason":"stop"}],"usage":{"prompt_tokens":300,"completion_tokens":200,"total_tokens":500}}))
    }}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
    let provider = tokio::spawn(async move {
        axum::serve(listener, provider).await.unwrap();
    });
    std::fs::write(
        paths.credentials.join("structured.token"),
        "local-controlled-structured-test-token-0001",
    )
    .unwrap();
    let mut config = InstanceConfig::load(&paths).unwrap();
    config.models.structured_formation=Some(serde_json::from_value(json!({"resource":{"id":"test-structured","provider":"test","protocol":"openai_chat_completions","endpoint":endpoint,"model":"test-structured","credential":{"file":"credentials/structured.token"}}})).unwrap());
    std::fs::write(&paths.config, toml::to_string_pretty(&config).unwrap()).unwrap();
    let app = App::open(paths.clone()).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.start_background(cancel.clone());
    let done = wait(&app, &job_id, "succeeded").await;
    let groups = done["groups"].as_array().unwrap();
    let states = groups
        .iter()
        .map(|g| g["state"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        [
            "applied", "conflict", "skipped", "applied", "applied", "applied"
        ]
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(done["model_calls"].as_array().unwrap().len(), 1);
    let found = app
        .query_structured(&ctx, query(&subject, "maple"))
        .await
        .unwrap();
    assert_eq!(found["hits"].as_array().unwrap().len(), 4);
    assert_eq!(
        app.concept_by_alias(&ctx, &subject.id, "Must rollback")
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let schema_id: CognitiveId = groups[4]["result"]["objects"][0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let source = app.structured_source(&ctx, &schema_id, None).await.unwrap();
    assert_eq!(source["closure"]["roots"].as_array().unwrap().len(), 2);
    assert_eq!(source["producer"]["state"], "succeeded");
    cancel.cancel();
    worker.await.unwrap();
    app.close().await;
    let app = App::open(paths).await.unwrap();
    let cancel = CancellationToken::new();
    let worker = app.start_background(cancel.clone());
    assert_eq!(
        app.structured_source(&ctx, &schema_id, None).await.unwrap()["closure"],
        source["closure"]
    );
    let reopened = wait(&app, &job_id, "succeeded").await;
    assert_eq!(done["groups"], reopened["groups"]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    cancel.cancel();
    worker.await.unwrap();
    app.close().await;
    provider.abort();
}
