use nous_domain::*;
use nous_storage::*;
use serde_json::json;
const NOW: &str = "2026-10-23T00:00:00Z";
fn directory() -> tempfile::TempDir {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/storage");
    std::fs::create_dir_all(&path).unwrap();
    tempfile::tempdir_in(path).unwrap()
}
async fn transaction(db: &Database) -> WriteTransaction {
    let BeginOperation::New(tx) = db
        .begin_operation(OperationKey {
            caller_id: "test".into(),
            operation: "write".into(),
            scope: "instance".into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: "request".into(),
        })
        .await
        .unwrap()
    else {
        panic!("new operation")
    };
    tx
}
async fn seed(db: &Database) -> Memory {
    let mut tx = transaction(db).await;
    let ns = NamespaceId::new();
    let subject = SubjectId::new();
    let material = MaterialId::new();
    let observation = ObservationId::new();
    tx.insert_namespace(&Namespace {
        id: ns.clone(),
        name: "research".into(),
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
        content_hash: "source-hash".into(),
        media_type: "text/plain".into(),
        byte_len: 4,
        source: Some("field-notes".into()),
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
            tags: vec!["coast".into()],
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
    let point = TimePoint {
        domain: "unix_ms".into(),
        coordinate: 1,
    };
    let memory = Memory {
        id: MemoryId::new(),
        namespace_id: ns,
        subject_id: subject,
        revision: 1,
        title: "Coastal wind".into(),
        content: "Wind crosses the harbor.".into(),
        epistemic_status: "reported".into(),
        observation_id: observation,
        material_id: material,
        model_call_id: None,
        origin: MemoryOrigin::Imported,
        times: CognitiveTimes {
            event: None,
            observation: None,
            formation: point.clone(),
            validity: None,
            recorded: point,
        },
        recorded_at: NOW.into(),
        deleted: false,
        lifecycle: MemoryLifecycle::new(1),
        commit_seq: tx.commit_seq(),
    };
    tx.save_memory(&memory, None).await.unwrap();
    tx.commit(json!({"id":memory.id})).await.unwrap();
    memory
}
fn task(memory: &Memory) -> JobRecord {
    JobRecord {
        id: ulid::Ulid::generate().to_string(),
        namespace_id: Some(memory.namespace_id.clone()),
        subject_id: Some(memory.subject_id.clone()),
        kind: "cognition.pipeline".into(),
        state: JobState::Queued,
        wait: None,
        checkpoint: None,
        result: None,
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    }
}
fn call(job: &JobRecord, step: &str) -> ModelCall {
    ModelCall {
        id: ModelCallId::new(),
        job_id: job.id.clone(),
        step_key: step.into(),
        namespace_id: job.namespace_id.clone().unwrap(),
        subject_id: job.subject_id.clone().unwrap(),
        attempt: 1,
        resource_id: "resource".into(),
        request: json!({"step":step}),
        response: None,
        state: ModelCallState::Started,
        error: None,
        cost_microunits: None,
        created_at: NOW.into(),
        completed_at: None,
    }
}
fn concept(memory: &Memory, sequence: i64) -> StructuredObject {
    StructuredObject {
        id: CognitiveId::new(),
        namespace_id: memory.namespace_id.clone(),
        subject_id: memory.subject_id.clone(),
        revision: 1,
        body: CognitiveContent::Concept(ConceptContent {
            label: "Harbor".into(),
            description: "A sheltered coastal place.".into(),
            aliases: vec!["Port".into()],
            canonical: None,
            members: vec![],
            lineage: vec![],
        }),
        evidence: vec![EvidenceLink {
            source: EvidenceSource::MemoryRevision {
                memory_id: memory.id.clone(),
                revision: 1,
            },
            polarity: EvidencePolarity::Support,
            relation: Some(EpistemicRelation::Supports),
            explanation: "field observation".into(),
        }],
        times: memory.times.clone(),
        producer_call_id: None,
        recorded_at: NOW.into(),
        commit_seq: sequence,
        deleted: false,
    }
}

#[tokio::test]
async fn shared_calls_keep_steps_budget_and_interrupted_attempts() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let memory = seed(&db).await;
    let job = task(&memory);
    let first = call(&job, "describe");
    let second = call(&job, "embed");
    let mut tx = transaction(&db).await;
    tx.insert_job_record(&job).await.unwrap();
    tx.insert_model_call(&first).await.unwrap();
    tx.insert_model_call(&second).await.unwrap();
    assert_eq!(
        tx.latest_model_call(&job.id, "describe")
            .await
            .unwrap()
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        tx.model_step_calls(&job.id, "embed").await.unwrap().len(),
        1
    );
    assert_eq!(
        tx.daily_model_calls("resource", "2026-10-23")
            .await
            .unwrap(),
        2
    );
    tx.commit(json!({"steps":2})).await.unwrap();
    let mut tx = transaction(&db).await;
    let duplicate = ModelCall {
        id: ModelCallId::new(),
        ..first.clone()
    };
    assert_eq!(
        tx.insert_model_call(&duplicate).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    db.close().await;
    let db = Database::open(&path).await.unwrap();
    assert_eq!(db.recover_interrupted_calls().await.unwrap(), 2);
    assert_eq!(
        db.latest_model_call(&job.id, "describe")
            .await
            .unwrap()
            .unwrap()
            .state,
        ModelCallState::Uncertain
    );
    let mut tx = transaction(&db).await;
    assert_eq!(
        tx.daily_model_calls("resource", "2026-10-23")
            .await
            .unwrap(),
        2
    );
    tx.rollback().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn typed_vector_revisions_keep_sources_and_retire_payload_copies() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let memory = seed(&db).await;
    let mut tx = transaction(&db).await;
    let object = concept(&memory, tx.commit_seq());
    tx.save_structured(&object, None).await.unwrap();
    let cutoff = tx.commit_seq();
    tx.commit(json!({"id":object.id})).await.unwrap();
    assert_eq!(
        db.source_closure(&memory.namespace_id, &object.id, None)
            .await
            .unwrap()
            .roots
            .len(),
        1
    );
    let mut tx = transaction(&db).await;
    let mut foreign = object.clone();
    foreign.id = CognitiveId::new();
    foreign.subject_id = SubjectId::new();
    foreign.commit_seq = tx.commit_seq();
    assert_eq!(
        tx.save_structured(&foreign, None).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    tx.rollback().await.unwrap();
    let job = task(&memory);
    let inputs = vec![
        EmbeddingInputItem::for_memory(&memory).unwrap(),
        EmbeddingInputItem::for_cognition(&object, None).unwrap(),
        EmbeddingInputItem::tag("coast").unwrap(),
    ];
    let run = EmbeddingRun {
        job: job.clone(),
        producer_signature: "producer".into(),
        space_id: "space".into(),
        inputs,
    };
    let mut tx = transaction(&db).await;
    tx.save_embedding_space(&EmbeddingSpace {
        id: "space".into(),
        resource_id: "resource".into(),
        provider_model: "actual-model".into(),
        dimension: 2,
        producer_signature: "producer".into(),
        modality: "text".into(),
    })
    .await
    .unwrap();
    tx.insert_job_record(&job).await.unwrap();
    tx.insert_embedding_run(&run).await.unwrap();
    tx.commit(json!({"job_id":job.id})).await.unwrap();
    let mut call = call(&job, "embed");
    let mut tx = transaction(&db).await;
    tx.insert_model_call(&call).await.unwrap();
    call.state = ModelCallState::Succeeded;
    call.response = Some(
        json!({"record":{"provider_model":"actual-model","usage":{"input_tokens":12}},"result":[[0.2,0.8],[0.2,0.8],[0.2,0.8]]}),
    );
    tx.update_model_call(&call).await.unwrap();
    let mut records = Vec::new();
    for input in &run.inputs {
        let record = EmbeddingRecord {
            namespace_id: memory.namespace_id.clone(),
            subject_id: memory.subject_id.clone(),
            space_id: "space".into(),
            target: input.target.clone(),
            source_hash: input.source_hash(),
            vector: vec![0.2, 0.8],
            model_call_id: call.id.clone(),
            commit_seq: tx.commit_seq(),
        };
        let altered = EmbeddingRecord {
            vector: vec![0.3, 0.7],
            ..record.clone()
        };
        assert_eq!(
            tx.save_embedding_record(&altered).await.unwrap_err().code,
            ErrorCode::Conflict
        );
        tx.save_embedding_record(&record).await.unwrap();
        records.push(record);
    }
    let mut done = job.clone();
    done.state = JobState::Succeeded;
    done.result = Some(json!({"call_id":call.id}));
    tx.update_job_record(&done).await.unwrap();
    tx.commit(json!({"adopted":3})).await.unwrap();
    let snapshot = db
        .cognitive_retrieval_snapshot(&memory.namespace_id, &memory.subject_id, "space", None, 100)
        .await
        .unwrap();
    assert_eq!(snapshot.structured.len(), 1);
    assert_eq!(
        snapshot.structured[0].materials[0].details.tags,
        vec!["coast"]
    );
    assert_eq!(snapshot.tag_embeddings.len(), 1);
    let mut tx = transaction(&db).await;
    let mut revised = object.clone();
    revised.revision = 2;
    revised.commit_seq = tx.commit_seq();
    tx.save_structured(&revised, Some(1)).await.unwrap();
    let copy = EmbeddingRecord {
        target: EmbeddingTarget::Cognition(revised.reference()),
        commit_seq: tx.commit_seq(),
        ..records[1].clone()
    };
    tx.save_embedding_record(&copy).await.unwrap();
    assert_eq!(
        tx.embeddings_for_cognition(&memory.namespace_id, &revised.reference())
            .await
            .unwrap()
            .len(),
        2
    );
    tx.commit(json!({"revision":2})).await.unwrap();
    let mut view = db.read_view().await.unwrap();
    let exact = view
        .cognitive_reference_snapshot(
            &memory.namespace_id,
            &memory.subject_id,
            "space",
            &object.reference(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(exact.structured.len(), 1);
    assert_eq!(exact.structured[0].object.revision, 1);
    assert_eq!(exact.structured[0].materials[0].details.tags, vec!["coast"]);
    assert_eq!(exact.tag_embeddings.len(), 1);
    assert!(exact.memories.is_empty());
    assert_eq!(
        view.cognitive_reference_snapshot(
            &memory.namespace_id,
            &memory.subject_id,
            "space",
            &revised.reference(),
            Some(cutoff)
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    let wrong_kind = CognitiveRef {
        kind: CognitiveKind::Episode,
        ..object.reference()
    };
    assert_eq!(
        view.cognitive_reference_snapshot(
            &memory.namespace_id,
            &memory.subject_id,
            "space",
            &wrong_kind,
            None
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::NotFound
    );
    let exact_memory = view
        .cognitive_reference_snapshot(
            &memory.namespace_id,
            &memory.subject_id,
            "space",
            &CognitiveRef::memory(&memory.id, 1),
            None,
        )
        .await
        .unwrap();
    assert_eq!(exact_memory.memories.len(), 1);
    assert!(exact_memory.structured.is_empty());
    view.finish().await.unwrap();
    let old = db
        .cognitive_retrieval_snapshot(
            &memory.namespace_id,
            &memory.subject_id,
            "space",
            Some(cutoff),
            100,
        )
        .await
        .unwrap();
    assert_eq!(old.structured[0].object.revision, 1);
    assert_eq!(
        db.embeddings_for_cognition(&memory.namespace_id, &revised.reference())
            .await
            .unwrap()
            .len(),
        2
    );
    let mut tx = transaction(&db).await;
    let mut trash = memory.clone();
    trash.revision = 2;
    trash.deleted = true;
    trash.commit_seq = tx.commit_seq();
    tx.save_memory(&trash, Some(1)).await.unwrap();
    tx.validate_embedding_input(&memory.namespace_id, &memory.subject_id, &run.inputs[0])
        .await
        .unwrap();
    tx.purge_memory(&memory.namespace_id, &memory.id, 2)
        .await
        .unwrap();
    tx.commit(json!({"purged":true})).await.unwrap();
    assert!(
        !db.source_closure(&memory.namespace_id, &object.id, None)
            .await
            .unwrap()
            .complete
    );
    assert!(
        db.embedding_run(&job.id)
            .await
            .unwrap()
            .inputs
            .iter()
            .filter(|item| item.target.reference().is_some())
            .all(|item| item.text.is_empty())
    );
    assert_eq!(
        db.model_call(&call.id).await.unwrap().response.unwrap()["redacted"],
        true
    );
    assert!(
        db.embeddings_for_cognition(&memory.namespace_id, &CognitiveRef::memory(&memory.id, 1))
            .await
            .unwrap()
            .is_empty()
    );
    db.close().await;
    let reopened = Database::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .structured_history(&memory.namespace_id, &object.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        reopened
            .embeddings_for_cognition(&memory.namespace_id, &object.reference())
            .await
            .unwrap()[0]
            .vector,
        records[1].vector
    );
    reopened.close().await;
}
