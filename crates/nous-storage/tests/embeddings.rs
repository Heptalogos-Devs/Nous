use nous_domain::*;
use nous_storage::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};

const NOW: &str = "2026-10-23T00:00:00Z";
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
fn directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/embeddings");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
async fn transaction(db: &Database) -> WriteTransaction {
    let BeginOperation::New(tx) = db
        .begin_operation(OperationKey {
            caller_id: "embedding-test".into(),
            operation: "test".into(),
            scope: "instance".into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: "nous.embedding.test.v1".into(),
        })
        .await
        .unwrap()
    else {
        panic!("new transaction")
    };
    tx
}
fn space() -> EmbeddingSpace {
    EmbeddingSpace {
        id: "space-test".into(),
        resource_id: "shared-resource".into(),
        provider_model: "actual-embedding-model".into(),
        dimension: 2,
        producer_signature: "producer-test".into(),
        modality: "text".into(),
    }
}
fn memory_input(memory: &Memory) -> EmbeddingInputItem {
    let text = format!("{}\n{}", memory.title, memory.content);
    EmbeddingInputItem {
        kind: EmbeddingKind::Memory,
        source_key: format!("memory:{}@{}", memory.id, memory.revision),
        source_hash: hash(&text),
        text,
        memory_id: Some(memory.id.clone()),
        memory_revision: Some(memory.revision),
    }
}
fn text_input(kind: EmbeddingKind, text: &str) -> EmbeddingInputItem {
    let source_hash = hash(text);
    let prefix = if kind == EmbeddingKind::Tag {
        "tag"
    } else {
        "query"
    };
    EmbeddingInputItem {
        kind,
        source_key: format!("{prefix}:{source_hash}"),
        source_hash,
        text: text.into(),
        memory_id: None,
        memory_revision: None,
    }
}
async fn seed(db: &Database) -> Memory {
    let mut tx = transaction(db).await;
    let ns = NamespaceId::new();
    let subject = SubjectId::new();
    let material = MaterialId::new();
    let observation = ObservationId::new();
    tx.insert_namespace(&Namespace {
        id: ns.clone(),
        name: "embedding sources".into(),
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_subject(&Subject {
        id: subject.clone(),
        namespace_id: ns.clone(),
        name: "reader".into(),
        capabilities: vec![Capability::Memory],
        revision: 1,
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.insert_material(&Material {
        id: material.clone(),
        namespace_id: ns.clone(),
        content_hash: hash("original material"),
        media_type: "text/plain".into(),
        byte_len: 17,
        source: Some("test:original".into()),
        event_time: None,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    tx.set_material_details(
        &ns,
        &material,
        &MaterialDetails {
            tags: vec!["海风".into()],
            ..MaterialDetails::default()
        },
    )
    .await
    .unwrap();
    tx.insert_observation(&Observation {
        id: observation.clone(),
        namespace_id: ns.clone(),
        subject_id: subject.clone(),
        material_id: material.clone(),
        kind: ObservationKind::Import,
        observed_at: None,
        recorded_at: NOW.into(),
        commit_seq: tx.commit_seq(),
    })
    .await
    .unwrap();
    let memory = Memory {
        id: MemoryId::new(),
        namespace_id: ns,
        subject_id: subject,
        revision: 1,
        title: "Derived title".into(),
        content: "derived memory-only secret".into(),
        epistemic_status: "reported".into(),
        observation_id: observation,
        material_id: material,
        model_call_id: None,
        prompt_version: None,
        origin: MemoryOrigin::Imported,
        times: CognitiveTimes {
            event: None,
            observation: None,
            formation: TimePoint {
                domain: "unix_ms".into(),
                coordinate: 1,
            },
            validity: None,
            recorded: TimePoint {
                domain: "unix_ms".into(),
                coordinate: 1,
            },
        },
        recorded_at: NOW.into(),
        deleted: false,
        commit_seq: tx.commit_seq(),
        lifecycle: MemoryLifecycle::new(1),
    };
    tx.save_memory(&memory, None).await.unwrap();
    tx.commit(json!({"memory_id":memory.id})).await.unwrap();
    memory
}
fn job(memory: &Memory, inputs: Vec<EmbeddingInputItem>) -> EmbeddingRun {
    let space = space();
    EmbeddingRun {
        job_id: ulid::Ulid::generate().to_string(),
        namespace_id: memory.namespace_id.clone(),
        subject_id: memory.subject_id.clone(),
        producer_signature: space.producer_signature,
        space_id: space.id,
        inputs,
        state: EmbeddingState::Queued,
        model_call_id: None,
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    }
}
async fn enqueue(tx: &mut WriteTransaction, run: &EmbeddingRun) {
    tx.insert_embedding_run(run).await.unwrap();
    tx.enqueue(JobEnvelope {
        id: run.job_id.clone(),
        kind: "embedding.run".into(),
        namespace_id: run.namespace_id.to_string(),
        subject_id: Some(run.subject_id.to_string()),
        expected_revision: None,
        payload: json!({"schema_version":1}),
    })
    .await
    .unwrap();
}
async fn reserve(
    db: &Database,
    memory: &Memory,
    inputs: Vec<EmbeddingInputItem>,
) -> (EmbeddingRun, ModelCall) {
    let mut tx = transaction(db).await;
    tx.save_embedding_space(&space()).await.unwrap();
    let mut run = job(memory, inputs);
    enqueue(&mut tx, &run).await;
    let call = ModelCall {
        id: ModelCallId::new(),
        job_id: run.job_id.clone(),
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        attempt: 1,
        resource_id: space().resource_id,
        request: json!({"schema_version":1,"inputs":run.inputs,"space":space()}),
        response: None,
        state: ModelCallState::Started,
        error: None,
        cost_microunits: None,
        created_at: NOW.into(),
        completed_at: None,
    };
    tx.insert_model_call(&call).await.unwrap();
    run.state = EmbeddingState::Running;
    run.model_call_id = Some(call.id.clone());
    tx.update_embedding_run(&run).await.unwrap();
    tx.commit(json!({"job_id":run.job_id,"count":run.inputs.len()}))
        .await
        .unwrap();
    (run, call)
}
fn record(
    run: &EmbeddingRun,
    call: &ModelCall,
    input: &EmbeddingInputItem,
    sequence: i64,
) -> EmbeddingRecord {
    EmbeddingRecord {
        namespace_id: run.namespace_id.clone(),
        subject_id: run.subject_id.clone(),
        space_id: run.space_id.clone(),
        kind: input.kind,
        source_key: input.source_key.clone(),
        source_hash: input.source_hash.clone(),
        memory_id: input.memory_id.clone(),
        memory_revision: input.memory_revision,
        vector: vec![0.25, 0.75],
        model_call_id: call.id.clone(),
        commit_seq: sequence,
    }
}
async fn adopt(
    db: &Database,
    run: &mut EmbeddingRun,
    call: &mut ModelCall,
) -> Vec<EmbeddingRecord> {
    let mut tx = transaction(db).await;
    call.state = ModelCallState::Succeeded;
    call.completed_at = Some(NOW.into());
    call.response = Some(
        json!({"schema_version":1,"record":{"provider_model":space().provider_model,"usage":{"input_tokens":4},"metadata":{"inputs":run.inputs}},"vectors":[[0.25,0.75]],"raw_output":"derived memory-only secret"}),
    );
    tx.update_model_call(call).await.unwrap();
    let mut records = vec![];
    for input in &run.inputs {
        let record = record(run, call, input, tx.commit_seq());
        tx.save_embedding_record(&record).await.unwrap();
        records.push(record);
    }
    run.state = EmbeddingState::Succeeded;
    tx.update_embedding_run(run).await.unwrap();
    tx.commit(json!({"job_id":run.job_id,"adopted":records.len()}))
        .await
        .unwrap();
    records
}

#[tokio::test]
async fn generic_embedding_job_and_formation_share_the_call_budget() {
    let directory = directory();
    let db = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let memory = seed(&db).await;
    let (run, mut call) = reserve(&db, &memory, vec![memory_input(&memory)]).await;
    let mut tx = transaction(&db).await;
    assert_eq!(
        tx.daily_model_calls(&space().resource_id, "2026-10-23")
            .await
            .unwrap(),
        1
    );
    call.state = ModelCallState::Uncertain;
    call.error = Some(Error::new(
        ErrorCode::DependencyUnavailable,
        "remote response was lost",
    ));
    tx.update_model_call(&call).await.unwrap();
    let formation = FormationRun {
        job_id: ulid::Ulid::generate().to_string(),
        namespace_id: memory.namespace_id.clone(),
        subject_id: memory.subject_id.clone(),
        observation_id: memory.observation_id.clone(),
        material_id: memory.material_id.clone(),
        subject_revision: 1,
        material_hash: hash("original material"),
        state: FormationState::Queued,
        model_call_id: None,
        memory_ids: vec![],
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    };
    tx.insert_formation(&formation).await.unwrap();
    tx.enqueue(JobEnvelope {
        id: formation.job_id.clone(),
        kind: "memory.form".into(),
        namespace_id: memory.namespace_id.to_string(),
        subject_id: Some(memory.subject_id.to_string()),
        expected_revision: Some(1),
        payload: json!({}),
    })
    .await
    .unwrap();
    let formation_call = ModelCall {
        id: ModelCallId::new(),
        job_id: formation.job_id.clone(),
        state: ModelCallState::Started,
        error: None,
        ..call.clone()
    };
    tx.insert_model_call(&formation_call).await.unwrap();
    assert_eq!(
        tx.daily_model_calls(&space().resource_id, "2026-10-23")
            .await
            .unwrap(),
        2
    );
    tx.commit(json!({"reserved":2})).await.unwrap();
    assert_eq!(
        db.model_call(&call.id).await.unwrap().state,
        ModelCallState::Uncertain
    );
    assert_eq!(db.model_calls(&run.job_id).await.unwrap().len(), 1);
    assert_eq!(db.model_calls(&formation.job_id).await.unwrap().len(), 1);
    db.close().await;
}

#[tokio::test]
async fn vectors_validate_adopt_once_and_reuse_content_across_lifecycle_revisions() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let memory = seed(&db).await;
    let inputs = vec![
        memory_input(&memory),
        text_input(EmbeddingKind::Tag, "海风"),
        text_input(EmbeddingKind::Query, "风筝"),
    ];
    let (mut run, mut call) = reserve(&db, &memory, inputs).await;
    for vector in [vec![0.25], vec![0.0, 0.0], vec![f32::NAN, 0.5]] {
        let mut tx = transaction(&db).await;
        let invalid = EmbeddingRecord {
            vector,
            ..record(&run, &call, &run.inputs[0], tx.commit_seq())
        };
        assert_eq!(
            tx.save_embedding_record(&invalid).await.unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        tx.rollback().await.unwrap();
    }
    let records = adopt(&db, &mut run, &mut call).await;
    let first_sequence = records[0].commit_seq;
    let snapshot = db
        .retrieval_snapshot(&memory.namespace_id, &memory.subject_id, &run.space_id, 10)
        .await
        .unwrap();
    assert!(snapshot.complete);
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.tag_embeddings.len(), 1);
    assert_eq!(
        snapshot.entries[0].embedding.as_ref().unwrap().vector,
        records[0].vector
    );
    assert_eq!(snapshot.entries[0].details.tags, vec!["海风"]);
    assert!(
        db.embedding_record(
            &memory.namespace_id,
            &SubjectId::new(),
            &run.space_id,
            EmbeddingKind::Memory,
            &records[0].source_key
        )
        .await
        .unwrap()
        .is_none()
    );
    let mut tx = transaction(&db).await;
    let mut replay = records[0].clone();
    replay.commit_seq = tx.commit_seq();
    tx.save_embedding_record(&replay).await.unwrap();
    tx.commit(json!({"replay":true})).await.unwrap();
    assert_eq!(
        db.embedding_record(
            &memory.namespace_id,
            &memory.subject_id,
            &run.space_id,
            EmbeddingKind::Memory,
            &records[0].source_key
        )
        .await
        .unwrap()
        .unwrap()
        .commit_seq,
        first_sequence
    );
    let mut tx = transaction(&db).await;
    let mut frozen = memory.clone();
    frozen.revision = 2;
    frozen.commit_seq = tx.commit_seq();
    frozen.lifecycle.activation = MemoryActivation::Frozen;
    tx.save_memory(&frozen, Some(1)).await.unwrap();
    let cloned = EmbeddingRecord {
        source_key: format!("memory:{}@2", memory.id),
        memory_revision: Some(2),
        commit_seq: tx.commit_seq(),
        ..records[0].clone()
    };
    tx.save_embedding_record(&cloned).await.unwrap();
    assert_eq!(
        tx.daily_model_calls(&space().resource_id, "2026-10-23")
            .await
            .unwrap(),
        1
    );
    tx.commit(json!({"reused":true})).await.unwrap();
    let snapshot = db
        .retrieval_snapshot(&memory.namespace_id, &memory.subject_id, &run.space_id, 10)
        .await
        .unwrap();
    assert_eq!(
        snapshot.entries[0].memory.lifecycle.activation,
        MemoryActivation::Frozen
    );
    assert_eq!(
        snapshot.entries[0]
            .embedding
            .as_ref()
            .unwrap()
            .memory_revision,
        Some(2)
    );
    assert_eq!(
        snapshot.entries[0]
            .embedding
            .as_ref()
            .unwrap()
            .model_call_id,
        call.id
    );
    let mut tx = transaction(&db).await;
    let changed = Memory {
        revision: 3,
        content: "new content".into(),
        commit_seq: tx.commit_seq(),
        ..frozen
    };
    tx.save_memory(&changed, Some(2)).await.unwrap();
    let stale = EmbeddingRecord {
        source_key: format!("memory:{}@3", memory.id),
        memory_revision: Some(3),
        commit_seq: tx.commit_seq(),
        ..records[0].clone()
    };
    assert!(tx.save_embedding_record(&stale).await.is_err());
    tx.rollback().await.unwrap();
    db.close().await;
    let db = Database::open(&path).await.unwrap();
    assert_eq!(
        db.embeddings_for_memory(&memory.namespace_id, &memory.id)
            .await
            .unwrap()
            .len(),
        2
    );
    db.close().await;
}

#[tokio::test]
async fn purge_cancels_embedding_batches_and_waiting_resume_preserves_outcomes() {
    let directory = directory();
    let path = directory.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let memory = seed(&db).await;
    let mut tx = transaction(&db).await;
    let survivor = Memory {
        id: MemoryId::new(),
        content: "independent surviving memory".into(),
        commit_seq: tx.commit_seq(),
        ..memory.clone()
    };
    tx.save_memory(&survivor, None).await.unwrap();
    tx.commit(json!({"survivor":survivor.id})).await.unwrap();
    let (mut run, mut call) = reserve(
        &db,
        &memory,
        vec![memory_input(&memory), memory_input(&survivor)],
    )
    .await;
    let records = adopt(&db, &mut run, &mut call).await;
    let mut tx = transaction(&db).await;
    let trash = Memory {
        revision: 2,
        deleted: true,
        commit_seq: tx.commit_seq(),
        ..memory.clone()
    };
    tx.save_memory(&trash, Some(1)).await.unwrap();
    tx.commit(json!({"trashed":true})).await.unwrap();
    let mut tx = transaction(&db).await;
    tx.purge_memory(&memory.namespace_id, &memory.id, 2)
        .await
        .unwrap();
    tx.commit(json!({"purged":true})).await.unwrap();
    assert!(
        db.embeddings_for_memory(&memory.namespace_id, &memory.id)
            .await
            .unwrap()
            .is_empty()
    );
    let cancelled = db.embedding_run(&run.job_id).await.unwrap();
    assert_eq!(cancelled.state, EmbeddingState::Cancelled);
    assert!(cancelled.inputs.iter().all(|input| input.text.is_empty()));
    let redacted = db.model_call(&call.id).await.unwrap();
    assert_eq!(redacted.state, ModelCallState::Succeeded);
    assert_eq!(
        redacted.response.as_ref().unwrap()["record"]["usage"]["input_tokens"],
        4
    );
    let ledger = serde_json::to_string(&redacted).unwrap();
    assert!(!ledger.contains(&memory.content));
    assert!(!ledger.contains(&survivor.content));
    assert_eq!(
        db.memory(&survivor.namespace_id, &survivor.id)
            .await
            .unwrap()
            .content,
        survivor.content
    );
    let mut tx = transaction(&db).await;
    let revised = Memory {
        revision: 2,
        commit_seq: tx.commit_seq(),
        ..survivor.clone()
    };
    tx.save_memory(&revised, Some(1)).await.unwrap();
    let reused = EmbeddingRecord {
        memory_revision: Some(2),
        source_key: format!("memory:{}@2", survivor.id),
        commit_seq: tx.commit_seq(),
        ..records[1].clone()
    };
    tx.save_embedding_record(&reused).await.unwrap();
    tx.commit(json!({"retained_source_reuse":true}))
        .await
        .unwrap();
    let mut tx = transaction(&db).await;
    let mut waiting = job(
        &revised,
        vec![text_input(EmbeddingKind::Query, "budget wait")],
    );
    waiting.state = EmbeddingState::WaitingForBudget;
    enqueue(&mut tx, &waiting).await;
    let mut configured = job(
        &revised,
        vec![text_input(EmbeddingKind::Query, "configuration wait")],
    );
    configured.state = EmbeddingState::WaitingForConfiguration;
    enqueue(&mut tx, &configured).await;
    let mut completed = job(
        &revised,
        vec![text_input(EmbeddingKind::Query, "completed work")],
    );
    completed.state = EmbeddingState::Succeeded;
    enqueue(&mut tx, &completed).await;
    tx.commit(json!({"waiting":true})).await.unwrap();
    let pool = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    for job in [
        &waiting.job_id,
        &configured.job_id,
        &completed.job_id,
        &run.job_id,
    ] {
        sqlx::query("UPDATE Jobs SET status='Done' WHERE id=?")
            .bind(job)
            .execute(&pool)
            .await
            .unwrap();
    }
    assert_eq!(db.resume_waiting_embeddings(false).await.unwrap(), 1);
    assert_eq!(
        db.embedding_run(&waiting.job_id).await.unwrap().state,
        EmbeddingState::WaitingForBudget
    );
    assert_eq!(db.resume_waiting_embeddings(true).await.unwrap(), 1);
    assert_eq!(
        db.embedding_run(&completed.job_id).await.unwrap().state,
        EmbeddingState::Succeeded
    );
    assert_eq!(
        db.embedding_run(&run.job_id).await.unwrap().state,
        EmbeddingState::Cancelled
    );
    pool.close().await;
    db.close().await;
}

#[tokio::test]
async fn different_batches_cannot_claim_overlapping_active_inputs() {
    let directory = directory();
    let db = Database::open(directory.path().join("authority.sqlite"))
        .await
        .unwrap();
    let memory = seed(&db).await;
    let shared = memory_input(&memory);
    let free = text_input(EmbeddingKind::Query, "new query input");
    let mut first = job(
        &memory,
        vec![shared.clone(), text_input(EmbeddingKind::Tag, "海风")],
    );
    let overlapping = job(&memory, vec![shared.clone(), free.clone()]);
    let mut tx = transaction(&db).await;
    tx.save_embedding_space(&space()).await.unwrap();
    enqueue(&mut tx, &first).await;
    tx.commit(json!({"job_id":first.job_id})).await.unwrap();
    let mut tx = transaction(&db).await;
    let claims = tx
        .embedding_claims(&memory.namespace_id, &memory.subject_id, &space().id)
        .await
        .unwrap();
    let claim = claims
        .iter()
        .find(|claim| claim.kind == shared.kind && claim.source_key == shared.source_key)
        .unwrap();
    assert_eq!(claim.job_id, first.job_id);
    assert_eq!(claim.source_hash, shared.source_hash);
    let error = tx.insert_embedding_run(&overlapping).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert!(error.message.contains(&first.job_id));
    tx.rollback().await.unwrap();
    let mut disjoint = job(&memory, vec![free]);
    let mut tx = transaction(&db).await;
    enqueue(&mut tx, &disjoint).await;
    tx.commit(json!({"job_id":disjoint.job_id})).await.unwrap();
    let mut tx = transaction(&db).await;
    first.state = EmbeddingState::Failed;
    disjoint.state = EmbeddingState::Succeeded;
    tx.update_embedding_run(&first).await.unwrap();
    tx.update_embedding_run(&disjoint).await.unwrap();
    enqueue(&mut tx, &overlapping).await;
    let claims = tx
        .embedding_claims(&memory.namespace_id, &memory.subject_id, &space().id)
        .await
        .unwrap();
    assert_eq!(claims.len(), 2);
    assert!(
        claims
            .iter()
            .all(|claim| claim.job_id == overlapping.job_id)
    );
    tx.commit(json!({"replacement_job_id":overlapping.job_id}))
        .await
        .unwrap();
    db.close().await;
}
