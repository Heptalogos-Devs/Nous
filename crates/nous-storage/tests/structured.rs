use nous_domain::*;
use nous_storage::*;
use serde_json::json;

const NOW: &str = "2026-10-23T00:00:00Z";

fn directory() -> tempfile::TempDir {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests/structured");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
async fn transaction(db: &Database) -> WriteTransaction {
    let BeginOperation::New(tx) = db
        .begin_operation(OperationKey {
            caller_id: "structured-test".into(),
            operation: "structured".into(),
            scope: "instance".into(),
            operation_id: ulid::Ulid::generate().to_string(),
            request_hash: "structured.test.v1".into(),
        })
        .await
        .unwrap()
    else {
        panic!("new transaction")
    };
    tx
}
fn times() -> CognitiveTimes {
    CognitiveTimes {
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
    }
}
struct Fixture {
    ns: NamespaceId,
    subject: SubjectId,
    foreign_subject: SubjectId,
    material: MaterialId,
    observation: ObservationId,
    memory: MemoryId,
    sequence: i64,
}
async fn fixture(db: &Database) -> Fixture {
    let mut tx = transaction(db).await;
    let ns = NamespaceId::new();
    let subject = SubjectId::new();
    let foreign_subject = SubjectId::new();
    let material = MaterialId::new();
    let observation = ObservationId::new();
    let memory = MemoryId::new();
    let sequence = tx.commit_seq();
    tx.insert_namespace(&Namespace {
        id: ns.clone(),
        name: "test".into(),
        revision: 1,
        commit_seq: sequence,
    })
    .await
    .unwrap();
    for id in [&subject, &foreign_subject] {
        tx.insert_subject(&Subject {
            id: id.clone(),
            namespace_id: ns.clone(),
            name: "reader".into(),
            capabilities: vec![Capability::Memory],
            revision: 1,
            commit_seq: sequence,
        })
        .await
        .unwrap();
    }
    tx.insert_material(&Material {
        id: material.clone(),
        namespace_id: ns.clone(),
        content_hash: "original".into(),
        media_type: "text/plain".into(),
        byte_len: 4,
        source: Some("notes".into()),
        event_time: None,
        recorded_at: NOW.into(),
        commit_seq: sequence,
    })
    .await
    .unwrap();
    tx.insert_observation(&Observation {
        id: observation.clone(),
        namespace_id: ns.clone(),
        subject_id: subject.clone(),
        material_id: material.clone(),
        kind: ObservationKind::Contact,
        observed_at: None,
        recorded_at: NOW.into(),
        commit_seq: sequence,
    })
    .await
    .unwrap();
    tx.save_memory(
        &Memory {
            id: memory.clone(),
            namespace_id: ns.clone(),
            subject_id: subject.clone(),
            revision: 1,
            title: "A source".into(),
            content: "An exact statement.".into(),
            epistemic_status: "observed".into(),
            observation_id: observation.clone(),
            material_id: material.clone(),
            model_call_id: None,
            prompt_version: None,
            origin: MemoryOrigin::Imported,
            times: times(),
            recorded_at: NOW.into(),
            deleted: false,
            lifecycle: MemoryLifecycle::new(1),
            commit_seq: sequence,
        },
        None,
    )
    .await
    .unwrap();
    tx.commit(json!({"fixture":true})).await.unwrap();
    Fixture {
        ns,
        subject,
        foreign_subject,
        material,
        observation,
        memory,
        sequence,
    }
}
fn support(f: &Fixture) -> EvidenceLink {
    EvidenceLink {
        source: EvidenceSource::MemoryRevision {
            memory_id: f.memory.clone(),
            revision: 1,
        },
        polarity: EvidencePolarity::Support,
        relation: Some(EpistemicRelation::Supports),
        explanation: "actual source".into(),
    }
}
fn concept(label: &str) -> CognitiveContent {
    CognitiveContent::Concept(ConceptContent {
        label: label.into(),
        description: "A scoped concept".into(),
        aliases: vec![format!("{label} alias")],
        canonical: None,
        members: vec![],
        lineage: vec![],
    })
}
fn object(f: &Fixture, body: CognitiveContent, seq: i64) -> StructuredObject {
    StructuredObject {
        id: CognitiveId::new(),
        namespace_id: f.ns.clone(),
        subject_id: f.subject.clone(),
        revision: 1,
        body,
        evidence: vec![support(f)],
        times: times(),
        producer_call_id: None,
        recorded_at: NOW.into(),
        commit_seq: seq,
        deleted: false,
    }
}

#[tokio::test]
async fn exact_revisions_share_rollback_scope_cas_and_deduplicated_roots() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let f = fixture(&db).await;
    let mut tx = transaction(&db).await;
    let mut first = object(&f, concept("Tide"), tx.commit_seq());
    first.evidence.push(EvidenceLink {
        source: EvidenceSource::Observation {
            observation_id: f.observation.clone(),
        },
        polarity: EvidencePolarity::Context,
        relation: None,
        explanation: "same source, independent role".into(),
    });
    tx.save_structured(&first, None).await.unwrap();
    let first_seq = tx.commit_seq();
    tx.commit(json!({"id":first.id})).await.unwrap();
    assert_eq!(
        db.concept_by_alias(&f.ns, &f.subject, " TIDE ALIAS ")
            .await
            .unwrap()
            .id,
        first.id
    );
    let closure = db.source_closure(&f.ns, &first.id, None).await.unwrap();
    assert!(closure.complete);
    assert_eq!(closure.roots.len(), 1);
    assert_eq!(closure.roots[0].material_id, f.material);
    assert_eq!(
        db.structured_revision(&f.ns, &first.id, 1)
            .await
            .unwrap()
            .evidence
            .len(),
        2
    );

    let mut tx = transaction(&db).await;
    let mut foreign = object(&f, concept("Foreign"), tx.commit_seq());
    foreign.subject_id = f.foreign_subject.clone();
    assert_eq!(
        tx.save_structured(&foreign, None).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&db).await;
    let rolled_back = object(&f, concept("Rollback"), tx.commit_seq());
    tx.save_structured(&rolled_back, None).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        db.structured_namespace(&rolled_back.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(db.status().await.unwrap().commit_seq, first_seq);

    let mut tx = transaction(&db).await;
    let mut retired = first.clone();
    retired.revision = 2;
    retired.deleted = true;
    retired.commit_seq = tx.commit_seq();
    tx.save_structured(&retired, Some(1)).await.unwrap();
    tx.commit(json!({"retired":true})).await.unwrap();
    assert!(
        db.structured_object(&f.ns, &first.id)
            .await
            .unwrap()
            .deleted
    );
    assert_eq!(db.source_closure(&f.ns,&first.id,Some(1)).await.unwrap().nodes.iter().find(|node|matches!(&node.source,EvidenceSource::CognitiveRevision{reference} if reference.id==first.id.as_str())).unwrap().status,SourceStatus::Retired);
    let snapshot = db
        .subject_structured_snapshot(
            &f.ns,
            &f.subject,
            Some(CognitiveKind::Concept),
            Some(first_seq),
            100,
        )
        .await
        .unwrap();
    assert_eq!(snapshot.objects[0].revision, 1);
    assert!(!snapshot.objects[0].deleted);
    let mut tx = transaction(&db).await;
    let mut restored = retired.clone();
    restored.revision = 3;
    restored.deleted = false;
    restored.commit_seq = tx.commit_seq();
    tx.save_structured(&restored, Some(2)).await.unwrap();
    tx.commit(json!({"restored":true})).await.unwrap();
    let mut tx = transaction(&db).await;
    let mut stale = retired;
    stale.commit_seq = tx.commit_seq();
    assert_eq!(
        tx.save_structured(&stale, Some(1)).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&db).await;
    let mut second = object(&f, concept("Bay"), tx.commit_seq());
    if let CognitiveContent::Concept(content) = &mut second.body {
        content.members.push(CognitiveRef::structured(
            CognitiveKind::Concept,
            &first.id,
            1,
        ));
    }
    tx.save_structured(&second, None).await.unwrap();
    tx.commit(json!({"id":second.id})).await.unwrap();
    let mut tx = transaction(&db).await;
    let mut cycle = restored;
    cycle.revision = 4;
    cycle.commit_seq = tx.commit_seq();
    if let CognitiveContent::Concept(content) = &mut cycle.body {
        content.members.push(CognitiveRef::structured(
            CognitiveKind::Concept,
            &second.id,
            1,
        ));
    }
    assert_eq!(
        tx.save_structured(&cycle, Some(3)).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        db.structured_history(&f.ns, &first.id).await.unwrap().len(),
        3
    );
    let mut tx = transaction(&db).await;
    let mut redirected = tx.structured_object(&f.ns, &first.id).await.unwrap();
    redirected.revision = 4;
    redirected.commit_seq = tx.commit_seq();
    if let CognitiveContent::Concept(content) = &mut redirected.body {
        content.canonical = Some(second.reference());
    }
    tx.save_structured(&redirected, Some(3)).await.unwrap();
    tx.commit(json!({"redirected":true})).await.unwrap();
    assert!(
        db.source_closure(&f.ns, &first.id, None)
            .await
            .unwrap()
            .complete
    );
    let mut tx = transaction(&db).await;
    let mut canonical_cycle = second.clone();
    canonical_cycle.revision = 2;
    canonical_cycle.commit_seq = tx.commit_seq();
    if let CognitiveContent::Concept(content) = &mut canonical_cycle.body {
        content.canonical = Some(redirected.reference());
    }
    assert_eq!(
        tx.save_structured(&canonical_cycle, Some(1))
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    db.close().await;
    let reopened = Database::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .structured_object(&f.ns, &first.id)
            .await
            .unwrap()
            .revision,
        4
    );
    reopened.close().await;
}

fn episode(f: &Fixture, start: i64, end: i64) -> CognitiveContent {
    CognitiveContent::Episode(EpisodeContent {
        title: "An experience".into(),
        track: "day".into(),
        experience: TimeRange {
            domain: "world".into(),
            start: Some(start),
            end: Some(end),
        },
        parent: None,
        members: vec![EpisodeMember {
            source: EvidenceSource::MemoryRevision {
                memory_id: f.memory.clone(),
                revision: 1,
            },
            role: "event".into(),
            ordinal: 0,
        }],
        boundary_explanation: "one interval".into(),
        lineage: vec![],
    })
}

#[tokio::test]
async fn episode_journal_constraints_and_purged_sources_preserve_exact_gaps() {
    let dir = directory();
    let db = Database::open(dir.path().join("authority.sqlite"))
        .await
        .unwrap();
    let f = fixture(&db).await;
    let mut tx = transaction(&db).await;
    let first = object(&f, episode(&f, 0, 10), tx.commit_seq());
    tx.save_structured(&first, None).await.unwrap();
    let episode_seq = tx.commit_seq();
    tx.commit(json!({"id":first.id})).await.unwrap();
    let mut tx = transaction(&db).await;
    let overlap = object(&f, episode(&f, 5, 15), tx.commit_seq());
    assert_eq!(
        tx.save_structured(&overlap, None).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&db).await;
    let mut child = object(&f, episode(&f, 1, 2), tx.commit_seq());
    if let CognitiveContent::Episode(content) = &mut child.body {
        content.parent = Some(CognitiveRef::structured(
            CognitiveKind::Episode,
            &first.id,
            1,
        ));
    }
    tx.save_structured(&child, None).await.unwrap();
    tx.commit(json!({"id":child.id})).await.unwrap();
    let journal_body = CognitiveContent::Journal(JournalContent {
        title: "Reflection".into(),
        narrative: "The observation remains a subject perspective.".into(),
        period: TimeRange {
            domain: "world".into(),
            start: Some(0),
            end: Some(10),
        },
        knowledge_cutoff: f.sequence,
        perspective: EpistemicStance {
            class: EpistemicClass::Narrative,
            attribution: Some("reader".into()),
            confidence: None,
        },
        points: vec![JournalPoint {
            role: JournalPointRole::Summary,
            text: "A sourced summary".into(),
            evidence: vec![support(&f)],
        }],
        episodes: vec![CognitiveRef::structured(
            CognitiveKind::Episode,
            &first.id,
            1,
        )],
        lineage: vec![],
    });
    let mut tx = transaction(&db).await;
    let mut journal = object(&f, journal_body, tx.commit_seq());
    assert_eq!(
        tx.save_structured(&journal, None).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&db).await;
    journal.commit_seq = tx.commit_seq();
    if let CognitiveContent::Journal(content) = &mut journal.body {
        content.knowledge_cutoff = episode_seq;
    }
    tx.save_structured(&journal, None).await.unwrap();
    tx.commit(json!({"id":journal.id})).await.unwrap();
    let closure = db.source_closure(&f.ns, &journal.id, None).await.unwrap();
    assert!(closure.complete);
    assert_eq!(closure.roots.len(), 1);
    let mut tx = transaction(&db).await;
    let schema_body = CognitiveContent::Schema(SchemaContent {
        title: "A bounded claim".into(),
        claim: "This pattern holds in the observed interval.".into(),
        scope: SchemaScope {
            description: "one setting".into(),
            about: vec![CognitiveRef::structured(
                CognitiveKind::Episode,
                &first.id,
                1,
            )],
            concepts: vec![],
            validity: None,
        },
        boundary: "Other settings remain open.".into(),
        formation_mode: FormationMode::Synthesized,
        epistemic: EpistemicStance {
            class: EpistemicClass::Inferred,
            attribution: None,
            confidence: Some(0.7),
        },
        support: vec![
            EvidenceSource::MemoryRevision {
                memory_id: f.memory.clone(),
                revision: 1,
            },
            EvidenceSource::Observation {
                observation_id: f.observation.clone(),
            },
        ],
        counterexamples: vec![],
        boundary_cases: vec![],
        lineage: vec![],
    });
    let mut schema = object(&f, schema_body, tx.commit_seq());
    assert_eq!(
        tx.save_structured(&schema, None).await.unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    tx.rollback().await.unwrap();
    let mut tx = transaction(&db).await;
    schema.commit_seq = tx.commit_seq();
    if let CognitiveContent::Schema(content) = &mut schema.body {
        content.formation_mode = FormationMode::Grounded;
        content.support.truncate(1);
        content.counterexamples.push(EvidenceSource::Observation {
            observation_id: f.observation.clone(),
        });
    }
    tx.save_structured(&schema, None).await.unwrap();
    let mut semantics = object(
        &f,
        CognitiveContent::MemorySemantics(MemorySemantics {
            memory_id: f.memory.clone(),
            memory_revision: 1,
            cognitive_role: CognitiveRole::Experiential,
            formation_mode: FormationMode::Grounded,
            epistemic: EpistemicStance {
                class: EpistemicClass::Observed,
                attribution: None,
                confidence: Some(1.0),
            },
            grounding_observation: Some(f.observation.clone()),
            concepts: vec![],
            qualifications: vec![],
        }),
        tx.commit_seq(),
    );
    semantics.evidence.push(EvidenceLink {
        source: EvidenceSource::Observation {
            observation_id: f.observation.clone(),
        },
        polarity: EvidencePolarity::Support,
        relation: Some(EpistemicRelation::Supports),
        explanation: "actual grounding contact".into(),
    });
    tx.save_structured(&semantics, None).await.unwrap();
    let association = object(
        &f,
        CognitiveContent::Association(AssociationContent {
            from: CognitiveRef::structured(CognitiveKind::Episode, &first.id, 1),
            to: CognitiveRef::structured(CognitiveKind::Journal, &journal.id, 1),
            relation: "summarized_by".into(),
            polarity: AssociationPolarity::Positive,
            basis_class: AssociationBasisClass::SourceEvidence,
            description: "Journal reflects the source episode.".into(),
        }),
        tx.commit_seq(),
    );
    tx.save_structured(&association, None).await.unwrap();
    tx.commit(json!({"schema":schema.id,"semantics":semantics.id,"association":association.id}))
        .await
        .unwrap();
    let loaded = db.structured_object(&f.ns, &schema.id).await.unwrap();
    let CognitiveContent::Schema(loaded) = loaded.body else {
        panic!("typed Schema")
    };
    assert_eq!(loaded.support.len(), 1);
    assert_eq!(loaded.counterexamples.len(), 1);
    assert!(matches!(
        db.structured_object(&f.ns, &semantics.id)
            .await
            .unwrap()
            .body,
        CognitiveContent::MemorySemantics(_)
    ));
    assert!(matches!(
        db.structured_object(&f.ns, &association.id)
            .await
            .unwrap()
            .body,
        CognitiveContent::Association(_)
    ));
    let mut tx = transaction(&db).await;
    let mut retired = tx.memory(&f.ns, &f.memory).await.unwrap();
    retired.revision = 2;
    retired.deleted = true;
    retired.commit_seq = tx.commit_seq();
    tx.save_memory(&retired, Some(1)).await.unwrap();
    tx.commit(json!({"trash":true})).await.unwrap();
    let mut tx = transaction(&db).await;
    tx.purge_memory(&f.ns, &f.memory, 2).await.unwrap();
    tx.commit(json!({"purged":true})).await.unwrap();
    let closure = db.source_closure(&f.ns, &journal.id, None).await.unwrap();
    assert!(!closure.complete);
    assert!(
        closure
            .nodes
            .iter()
            .any(|node| node.status == SourceStatus::Missing
                && matches!(node.source, EvidenceSource::MemoryRevision { .. }))
    );
    assert_eq!(
        db.structured_revision(&f.ns, &first.id, 1)
            .await
            .unwrap()
            .revision,
        1
    );
    db.close().await;
}

#[tokio::test]
async fn structured_proposal_groups_resume_without_another_model_call() {
    let dir = directory();
    let path = dir.path().join("authority.sqlite");
    let db = Database::open(&path).await.unwrap();
    let f = fixture(&db).await;
    let mut tx = transaction(&db).await;
    let mut run = StructuredFormationRun {
        job_id: ulid::Ulid::generate().to_string(),
        namespace_id: f.ns.clone(),
        subject_id: f.subject.clone(),
        state: FormationState::Queued,
        input: json!({"schema_version":1,"sources":[support(&f).source],"slots":[],"knowledge_cutoff":f.sequence}),
        output: None,
        model_call_id: None,
        error: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
    };
    tx.insert_structured_formation(&run).await.unwrap();
    tx.enqueue(JobEnvelope {
        id: run.job_id.clone(),
        kind: "structured.form.v1".into(),
        namespace_id: f.ns.to_string(),
        subject_id: Some(f.subject.to_string()),
        expected_revision: None,
        payload: json!({}),
    })
    .await
    .unwrap();
    tx.commit(json!({"job":run.job_id})).await.unwrap();
    let mut tx = transaction(&db).await;
    let call = ModelCall {
        id: ModelCallId::new(),
        job_id: run.job_id.clone(),
        namespace_id: f.ns.clone(),
        subject_id: f.subject.clone(),
        attempt: 1,
        resource_id: "shared-model".into(),
        request: run.input.clone(),
        response: Some(json!({"schema_version":1,"proposal":"a,b,c"})),
        state: ModelCallState::Succeeded,
        error: None,
        cost_microunits: None,
        created_at: NOW.into(),
        completed_at: Some(NOW.into()),
    };
    tx.insert_model_call(&call).await.unwrap();
    run.model_call_id = Some(call.id.clone());
    run.output = Some(json!({"groups":["a","b","c"]}));
    run.state = FormationState::Running;
    tx.update_structured_formation(&run).await.unwrap();
    let mut groups = (0..3)
        .map(|index| StructuredFormationGroup {
            key: ["a", "b", "c"][index].into(),
            ordinal: index as i64,
            depends_on: if index == 2 { vec!["b".into()] } else { vec![] },
            state: StructuredGroupState::Pending,
            result: None,
            error: None,
            commit_seq: None,
        })
        .collect::<Vec<_>>();
    tx.insert_structured_groups(&run.job_id, &groups)
        .await
        .unwrap();
    assert_eq!(
        tx.daily_model_calls("shared-model", "2026-10-23")
            .await
            .unwrap(),
        1
    );
    tx.commit(json!({"proposal":true})).await.unwrap();

    let mut tx = transaction(&db).await;
    let accepted = object(&f, concept("Group result"), tx.commit_seq());
    tx.save_structured(&accepted, None).await.unwrap();
    let mut outcome = groups[0].clone();
    outcome.state = StructuredGroupState::Applied;
    outcome.result = Some(json!({"id":accepted.id}));
    outcome.commit_seq = Some(tx.commit_seq());
    tx.update_structured_group(&run.job_id, &outcome)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        db.structured_namespace(&accepted.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        db.structured_formation_groups(&run.job_id).await.unwrap()[0].state,
        StructuredGroupState::Pending
    );
    let mut tx = transaction(&db).await;
    let mut accepted = accepted;
    accepted.commit_seq = tx.commit_seq();
    tx.save_structured(&accepted, None).await.unwrap();
    outcome.commit_seq = Some(tx.commit_seq());
    tx.update_structured_group(&run.job_id, &outcome)
        .await
        .unwrap();
    tx.commit(json!({"group":"a","id":accepted.id}))
        .await
        .unwrap();
    db.close().await;
    let db = Database::open(&path).await.unwrap();
    assert!(
        db.structured_formation(&run.job_id)
            .await
            .unwrap()
            .output
            .is_some()
    );
    assert_eq!(db.model_calls(&run.job_id).await.unwrap().len(), 1);
    assert_eq!(
        db.structured_formation_groups(&run.job_id).await.unwrap()[0],
        outcome
    );
    let mut tx = transaction(&db).await;
    groups[1].state = StructuredGroupState::Conflict;
    groups[1].error = Some(Error::new(ErrorCode::Conflict, "target revision changed"));
    groups[1].commit_seq = Some(tx.commit_seq());
    tx.update_structured_group(&run.job_id, &groups[1])
        .await
        .unwrap();
    tx.commit(json!({"group":"b","status":"conflict"}))
        .await
        .unwrap();
    let mut tx = transaction(&db).await;
    groups[2].state = StructuredGroupState::Skipped;
    groups[2].error = Some(Error::new(
        ErrorCode::Conflict,
        "dependency b did not apply",
    ));
    groups[2].commit_seq = Some(tx.commit_seq());
    tx.update_structured_group(&run.job_id, &groups[2])
        .await
        .unwrap();
    run.state = FormationState::Succeeded;
    tx.update_structured_formation(&run).await.unwrap();
    assert_eq!(
        tx.daily_model_calls("shared-model", "2026-10-23")
            .await
            .unwrap(),
        1
    );
    tx.commit(json!({"finished":true})).await.unwrap();
    assert_eq!(
        db.structured_formation(&run.job_id).await.unwrap().state,
        FormationState::Succeeded
    );
    assert_eq!(
        db.structured_history(&f.ns, &accepted.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.model_calls(&run.job_id).await.unwrap().len(), 1);
    db.close().await;
}

#[tokio::test]
async fn purge_retires_captured_direct_and_transitive_inputs_and_rejects_late_writes() {
    let dir = directory();
    let db = Database::open(dir.path().join("authority.sqlite"))
        .await
        .unwrap();
    let f = fixture(&db).await;
    let memory = db.memory(&f.ns, &f.memory).await.unwrap();
    let private = memory.content.clone();
    let mut tx = transaction(&db).await;
    let mut typed = object(&f, concept("Independent owner"), tx.commit_seq());
    if let CognitiveContent::Concept(content) = &mut typed.body {
        content.description = private.clone();
    }
    tx.save_structured(&typed, None).await.unwrap();
    tx.commit(json!({"id":typed.id})).await.unwrap();
    let mut runs = Vec::new();
    let mut calls = Vec::new();
    for (ordinal, source, text) in [
        (
            0,
            EvidenceSource::MemoryRevision {
                memory_id: f.memory.clone(),
                revision: 1,
            },
            serde_json::to_string(&memory).unwrap(),
        ),
        (
            1,
            EvidenceSource::CognitiveRevision {
                reference: typed.reference(),
            },
            serde_json::to_string(&typed).unwrap(),
        ),
        (
            2,
            EvidenceSource::Material {
                material_id: f.material.clone(),
            },
            "Original immutable material".into(),
        ),
    ] {
        let mut tx = transaction(&db).await;
        let mut run = StructuredFormationRun {
            job_id: ulid::Ulid::generate().to_string(),
            namespace_id: f.ns.clone(),
            subject_id: f.subject.clone(),
            state: FormationState::Queued,
            input: json!({"namespace_id":f.ns,"subject_id":f.subject,"knowledge_cutoff":tx.commit_seq()-1,"sources":[{"key":"s0","source":source,"text":text,"truncated":false}],"slots":[]}),
            output: None,
            model_call_id: None,
            error: None,
            created_at: NOW.into(),
            updated_at: NOW.into(),
        };
        tx.insert_structured_formation(&run).await.unwrap();
        tx.enqueue(JobEnvelope {
            id: run.job_id.clone(),
            kind: "cognition.form".into(),
            namespace_id: f.ns.to_string(),
            subject_id: Some(f.subject.to_string()),
            expected_revision: None,
            payload: json!({"schema_version":1}),
        })
        .await
        .unwrap();
        tx.commit(json!(run)).await.unwrap();
        if ordinal < 2 {
            let mut tx = transaction(&db).await;
            let succeeded = ordinal == 0;
            let call=ModelCall{id:ModelCallId::new(),job_id:run.job_id.clone(),namespace_id:f.ns.clone(),subject_id:f.subject.clone(),attempt:1,resource_id:"shared-model".into(),request:json!({"schema_version":1,"role":"structured_formation","resource":{"id":"shared-model"},"rendered_user":run.input}),response:succeeded.then(||json!({"record":{"provider_model":"provider-model","usage":{"input_tokens":17},"raw_output":private,"provider_response":{"text":private}},"proposal":{"description":private}})),state:if succeeded{ModelCallState::Succeeded}else{ModelCallState::Started},error:None,cost_microunits:Some(9),created_at:NOW.into(),completed_at:succeeded.then(||NOW.into())};
            tx.insert_model_call(&call).await.unwrap();
            run.model_call_id = Some(call.id.clone());
            run.state = FormationState::Running;
            if succeeded {
                run.output = Some(json!({"groups":[{"text":private}]}));
            }
            tx.update_structured_formation(&run).await.unwrap();
            if succeeded {
                tx.insert_structured_groups(
                    &run.job_id,
                    &[
                        StructuredFormationGroup {
                            key: "done".into(),
                            ordinal: 0,
                            depends_on: vec![],
                            state: StructuredGroupState::Pending,
                            result: None,
                            error: None,
                            commit_seq: None,
                        },
                        StructuredFormationGroup {
                            key: "pending".into(),
                            ordinal: 1,
                            depends_on: vec![],
                            state: StructuredGroupState::Pending,
                            result: None,
                            error: None,
                            commit_seq: None,
                        },
                    ],
                )
                .await
                .unwrap();
                let mut done =
                    tx.structured_formation_groups(&run.job_id).await.unwrap()[0].clone();
                done.state = StructuredGroupState::Applied;
                done.result = Some(json!({"objects":[typed]}));
                done.commit_seq = Some(tx.commit_seq());
                tx.update_structured_group(&run.job_id, &done)
                    .await
                    .unwrap();
            }
            tx.commit(json!({"job_id":run.job_id})).await.unwrap();
            calls.push(call);
        }
        runs.push(run);
    }
    let before = db.model_calls(&runs[0].job_id).await.unwrap().len()
        + db.model_calls(&runs[1].job_id).await.unwrap().len();
    let mut tx = transaction(&db).await;
    let mut memory = tx.memory(&f.ns, &f.memory).await.unwrap();
    memory.revision = 2;
    memory.deleted = true;
    memory.commit_seq = tx.commit_seq();
    tx.save_memory(&memory, Some(1)).await.unwrap();
    tx.purge_memory(&f.ns, &f.memory, 2).await.unwrap();
    tx.commit(json!({"purged":true})).await.unwrap();
    for original in &runs[..2] {
        let retired = db.structured_formation(&original.job_id).await.unwrap();
        assert!(retired.is_retired());
        assert_eq!(retired.state, FormationState::Cancelled);
        assert_eq!(retired.input["redacted"], true);
        assert!(!serde_json::to_string(&retired).unwrap().contains(&private));
        let mut tx = transaction(&db).await;
        assert_eq!(
            tx.update_structured_formation(original)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        tx.rollback().await.unwrap();
    }
    assert!(
        !db.structured_formation(&runs[2].job_id)
            .await
            .unwrap()
            .is_retired()
    );
    let groups = db
        .structured_formation_groups(&runs[0].job_id)
        .await
        .unwrap();
    assert_eq!(groups[0].state, StructuredGroupState::Applied);
    assert_eq!(
        groups[0].result.as_ref().unwrap()["objects"][0]["id"],
        json!(typed.id)
    );
    assert_eq!(groups[1].state, StructuredGroupState::Skipped);
    assert!(!serde_json::to_string(&groups).unwrap().contains(&private));
    let mut tx = transaction(&db).await;
    let mut stale_group = groups[1].clone();
    stale_group.state = StructuredGroupState::Applied;
    stale_group.result = Some(json!({"text":private}));
    stale_group.commit_seq = Some(tx.commit_seq());
    assert_eq!(
        tx.update_structured_group(&runs[0].job_id, &stale_group)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    tx.rollback().await.unwrap();
    for old in &calls {
        let current = db.model_call(&old.id).await.unwrap();
        assert_eq!(current.request["redacted"], true);
        assert!(!serde_json::to_string(&current).unwrap().contains(&private));
        assert_eq!(current.state, old.state);
        assert_eq!(current.cost_microunits, old.cost_microunits);
        let mut tx = transaction(&db).await;
        assert_eq!(
            tx.update_model_call(old).await.unwrap_err().code,
            ErrorCode::Conflict
        );
        tx.rollback().await.unwrap();
    }
    assert_eq!(
        db.model_call(&calls[0].id).await.unwrap().response.unwrap()["record"]["usage"]["input_tokens"],
        17
    );
    assert_eq!(
        db.model_calls(&runs[0].job_id).await.unwrap().len()
            + db.model_calls(&runs[1].job_id).await.unwrap().len(),
        before
    );
    let mut tx = transaction(&db).await;
    assert_eq!(
        tx.daily_model_calls("shared-model", "2026-10-23")
            .await
            .unwrap(),
        2
    );
    tx.rollback().await.unwrap();
    for original in &runs[..2] {
        let mut fresh = original.clone();
        fresh.job_id = ulid::Ulid::generate().to_string();
        fresh.state = FormationState::Queued;
        fresh.model_call_id = None;
        fresh.output = None;
        let mut tx = transaction(&db).await;
        assert_eq!(
            tx.insert_structured_formation(&fresh)
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        tx.rollback().await.unwrap();
    }
    let owner = db.structured_object(&f.ns, &typed.id).await.unwrap();
    assert!(serde_json::to_string(&owner).unwrap().contains(&private));
    assert!(
        !db.source_closure(&f.ns, &typed.id, None)
            .await
            .unwrap()
            .complete
    );
    let pool = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("authority.sqlite"))
            .create_if_missing(false),
    )
    .await
    .unwrap();
    let receipts: Vec<String> = sqlx::query_scalar(
        "SELECT receipt FROM operation_receipts WHERE json_extract(receipt,'$.result.job_id')=?",
    )
    .bind(&runs[0].job_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!receipts.is_empty());
    assert!(receipts.iter().all(|receipt| !receipt.contains(&private)));
    pool.close().await;
    db.close().await;
}
