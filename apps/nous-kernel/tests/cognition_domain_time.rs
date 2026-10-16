// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{Duration, Utc};
use nous_core::*;
use nous_runtime::ClockChange;
use test_support::*;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real PG journey proves three owner commits and durable formation replay under a paused world clock"
)]
async fn owner_formation_is_world_time_and_recovered_result_keeps_its_original_stamp() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(nous_subject::CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            metadata: serde_json::json!({}),
            capabilities: Some(nous_subject::SubjectCapabilities {
                memory: true,
                self_cognition: true,
                social_cognition: true,
                motivation: false,
            }),
            cognitive_seed: Some(nous_subject::CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            }),
        })
        .await
        .unwrap()
        .subject_id;
    let world = runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Configure {
                time_domain_ref: "time:course".into(),
                rate: 0.0,
                world_time: Some(Utc::now() + Duration::days(2)),
            },
        )
        .await
        .unwrap();
    let source = runtime
        .material
        .record_observation(nous_material::ObservationInput {
            subject,
            session: None,
            occurrence: nous_material::OccurrenceDescriptor {
                occurred_time_domain_ref: "time:real".into(),
                occurred_time: TemporalExtent::Unknown,
                source_class: SourceClass::Message,
                external_object_ref: None,
                observed_at: None,
                conversation_ref: Some("conversation:fixture:testing".into()),
                actor_entity_ref: Some(EntityRef::new("entity:fixture:author").unwrap()),
                context: serde_json::json!({}),
            },
            material: nous_material::ObservationMaterial::InlineText {
                text: "I prefer careful tests; this room calls a successful check green.".into(),
                media_type: "text/plain".into(),
            },
            entities: vec![],
            runtime: Default::default(),
        })
        .await
        .unwrap();
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "A real source supports careful tests",
        ))
        .await
        .unwrap();
    assert_eq!(memory.revision.formed_time_domain_ref, "time:course");
    assert_eq!(memory.revision.formed_at, world.world_now);
    assert!(
        (memory.revision.recorded_at - Utc::now())
            .num_seconds()
            .abs()
            < 2
    );
    let basis = memory.basis.clone();
    let own = runtime
        .require_self()
        .unwrap()
        .adopt(nous_self::AdoptSelf {
            subject,
            operation_id: OperationId::new(),
            reason: "Grounded fixture judgment".into(),
            content: nous_self::SelfContent::SelfFacet(nous_self::SelfFacetContent {
                valid_time_domain_ref: "time:course".into(),
                kind: "preference".into(),
                statement: "I prefer careful tests".into(),
                epistemic_class: EpistemicClass::Narrative,
                scope: nous_self::SelfScope {
                    description: "General testing".into(),
                    ..Default::default()
                },
                stance: "adopted".into(),
                exceptions: vec![],
                basis: basis.clone(),
                valid_time: TemporalExtent::Interval {
                    start: Some(world.world_now),
                    end: Some(world.world_now + Duration::minutes(1)),
                },
                producer: None,
            }),
        })
        .await
        .unwrap();
    assert_eq!(own.formed_time_domain_ref, "time:course");
    assert_eq!(own.formed_at, world.world_now);
    let read = nous_runtime::ContextResolver::source_read_view(
        &runtime,
        subject,
        &SourceReadScope {
            subject_id: Some(subject),
            source_refs: vec![CognitiveRef::Occurrence(source.occurrence.occurrence_id)],
            origin_conversation_refs: vec![],
        },
    )
    .await
    .unwrap();
    let scope = nous_runtime::InvocationScope {
        purpose: "reflection".into(),
        conversation: "conversation:fixture:testing".into(),
        ..Default::default()
    };
    let captured_clock = runtime.cognition.read_clock(subject).await.unwrap();
    let context = runtime
        .require_self()
        .unwrap()
        .automatic_context(
            subject,
            nous_runtime::AutomaticContextRequest {
                clock: &captured_clock,
                scope: &scope,
                max_items: 4,
                work_context: None,
                materialize: true,
                max_bytes: 8192,
                ranked: &[],
            },
            &read,
        )
        .await
        .unwrap();
    assert_eq!(context.sources.len(), 1);
    assert_eq!(context.sources[0].reference, own.reference());
    let social = runtime
        .require_social()
        .unwrap()
        .adopt(nous_social::AdoptSocial {
            subject,
            operation_id: OperationId::new(),
            reason: "Actual fixture expression".into(),
            content: nous_social::SocialContent::LanguageConvention(
                nous_social::ConventionContent {
                    valid_time_domain_ref: "time:real".into(),
                    expression: "green".into(),
                    variants: vec![],
                    case_insensitive: false,
                    meaning: "A successful check".into(),
                    referent: None,
                    scope: nous_social::SocialScope {
                        description: "The actual fixture room".into(),
                        conversation: Some("conversation:fixture:testing".into()),
                        ..Default::default()
                    },
                    pragmatic_use: "Report an actual test result".into(),
                    judgment: nous_social::SocialJudgment {
                        state: "supported".into(),
                        explanation: "The selected source explicitly defines the phrase".into(),
                        estimate: None,
                        estimate_method: None,
                    },
                    basis,
                    valid_time: TemporalExtent::Unknown,
                    producer: None,
                },
            ),
        })
        .await
        .unwrap();
    assert_eq!(social.formed_time_domain_ref, "time:course");
    assert_eq!(social.formed_at, world.world_now);
    let nous_social::SocialContent::LanguageConvention(mut historical) = social.content.clone()
    else {
        unreachable!()
    };
    historical.expression = "historical green".into();
    historical.valid_time_domain_ref = "time:course".into();
    historical.valid_time = TemporalExtent::Interval {
        start: Some(world.world_now),
        end: Some(world.world_now + Duration::hours(1)),
    };
    let historical = runtime
        .require_social()
        .unwrap()
        .adopt(nous_social::AdoptSocial {
            subject,
            operation_id: OperationId::new(),
            reason: "A dated supported convention".into(),
            content: nous_social::SocialContent::LanguageConvention(historical),
        })
        .await
        .unwrap();
    let nano_extent = serde_json::json!({"kind":"instant","at":"2026-10-16T00:00:00.999999999Z"});
    let nano_range = serde_json::json!({"time_domain_ref":"time:course","predicate":{"kind":"range","start":"2026-10-16T00:00:00Z","end":"2026-10-16T00:00:01Z"}});
    let nano_match: bool =
        sqlx::query_scalar("SELECT domain_temporal_matches($1,'time:course',$2)")
            .bind(&nano_extent)
            .bind(&nano_range)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    assert!(
        nano_match,
        "nanosecond must not round across the exclusive range end"
    );
    let cross_domain: bool =
        sqlx::query_scalar("SELECT domain_temporal_matches($1,'time:other',$2)")
            .bind(&nano_extent)
            .bind(&nano_range)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    assert!(!cross_domain);
    let past_predicate = [DomainTimePredicate {
        time_domain_ref: "time:course".into(),
        predicate: TimePredicate::Range {
            start: Some(world.world_now),
            end: Some(world.world_now + Duration::minutes(30)),
        },
    }];
    let historical_scope = historical.content.scope().clone();
    let authorization = SourceReadScope {
        subject_id: Some(subject),
        source_refs: vec![CognitiveRef::Occurrence(source.occurrence.occurrence_id)],
        origin_conversation_refs: vec![],
    };
    let later = DomainStamp {
        time_domain_ref: "time:course".into(),
        at: world.world_now + Duration::days(1),
        recorded_at: Utc::now(),
    };
    let selected = runtime
        .require_social()
        .unwrap()
        .select(
            subject,
            nous_social::SocialSelectionRequest {
                scope: &historical_scope,
                source_read: &authorization,
                max_items: 1,
                max_bytes: 65536,
                after: None,
                expression: None,
                endpoint: None,
                counterpart: None,
                at: Some(later),
                valid_predicates: Some(&past_predicate),
                as_of: None,
                history: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(selected.records.len(), 1);
    assert_eq!(selected.records[0].reference(), historical.reference());
    let clock = runtime.clocks.read(subject).await.unwrap();
    let stamp = DomainStamp {
        time_domain_ref: clock.time_domain_ref,
        at: clock.world_now,
        recorded_at: clock.recorded_at,
    };
    let operation = OperationId::new();
    runtime
        .store
        .save_model_operation(
            &nous_persistence::ModelOperationIdentity {
                subject,
                owner: "memory".into(),
                operation_key: operation.0.to_string(),
                semantic_digest: "deterministic-retained-result-fixture".into(),
            },
            Some(&ModelResultContext {
                cognitive_formed_at: Some(stamp.clone()),
                ..Default::default()
            }),
            Some(&ResultPayload {
                payload: serde_json::json!({"fixture":"already-generated deterministic result"}),
                ..Default::default()
            }),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Advance { seconds: 3600.0 },
        )
        .await
        .unwrap();
    let reopened = open_runtime(&url, &root).await;
    let expired_clock = reopened.cognition.read_clock(subject).await.unwrap();
    let expired = reopened
        .require_self()
        .unwrap()
        .automatic_context(
            subject,
            nous_runtime::AutomaticContextRequest {
                clock: &expired_clock,
                scope: &scope,
                max_items: 4,
                work_context: None,
                materialize: true,
                max_bytes: 8192,
                ranked: &[],
            },
            &read,
        )
        .await
        .unwrap();
    assert!(expired.sources.is_empty());

    let recovered = reopened
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            operation,
            "Retained deterministic proposal",
        ))
        .await
        .unwrap();
    assert_eq!(recovered.revision.formed_at, stamp.at);
    assert_eq!(
        recovered.revision.formed_recorded_at,
        chrono::SubsecRound::round_subsecs(stamp.recorded_at, 6)
    );
    assert_eq!(
        recovered.revision.formed_time_domain_ref,
        stamp.time_domain_ref
    );
    assert!(
        (recovered.revision.recorded_at - Utc::now())
            .num_seconds()
            .abs()
            < 2
    );
}

#[tokio::test]
async fn migration_upgrades_only_known_canonical_metadata_and_preserves_result_identity() {
    let (_root, url, _postgres) = database().await;
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let migrations = sqlx::migrate!("../../crates/persistence/migrations");
    migrations.run_to(14, &pool).await.unwrap();
    let subject = SubjectId::new();
    sqlx::query(
        "INSERT INTO subjects(subject_id,metadata,created_at) VALUES($1,'{}',clock_timestamp())",
    )
    .bind(subject.0)
    .execute(&pool)
    .await
    .unwrap();
    let at = "2026-10-15T12:00:00Z";
    let recorded = "2026-10-15T12:00:01Z";
    let record = serde_json::json!({"formedAt":at,"recordedAt":recorded,"revisionId":"an-opaque-fixture-ref", "content":{"facet":{"validTime":{"instant":at}}}});
    let proposal = serde_json::json!({"payload":{"formedAt":at,"plan":{"currentSelf":[["S1",record]],"currentSocial":[],"currentMemory":[],"currentMotivation":[]},"output":{"verbatim":"Do not rewrite formedAt or any raw source","validTime":{"kind":"instant","at":at}}}});
    let outcome = serde_json::json!({"payload":{"result":{"actions":[{"selfRecord":record}]},"proposal":proposal["payload"]}});
    sqlx::query("INSERT INTO model_results(subject_id,owner,operation_key,semantic_digest,context,proposal,outcome,created_at,updated_at) VALUES($1,'subject_reflection','fixture','unchanged-digest',$2,$3,$4,$5::timestamptz,$5::timestamptz)")
        .bind(subject.0).bind(serde_json::json!({"dependencies":[],"cognitive_formed_at":at,"maintenance_trigger":null})).bind(&proposal).bind(&outcome).bind(recorded).execute(&pool).await.unwrap();
    migrations.run(&pool).await.unwrap();
    let (digest, context, actual_proposal, actual_outcome): (
        String,
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT semantic_digest,context,proposal,outcome FROM model_results WHERE subject_id=$1",
    )
    .bind(subject.0)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(digest, "unchanged-digest");
    assert_eq!(
        actual_proposal["payload"]["plan"]["currentSelf"][0][1]["content"]["facet"]["validTime"]["timeDomainRef"],
        "time:real"
    );
    let context: nous_core::ModelResultContext = serde_json::from_value(context).unwrap();
    assert_eq!(
        context.cognitive_formed_at.unwrap().time_domain_ref,
        "time:real"
    );
    assert_eq!(
        actual_proposal["payload"]["plan"]["currentSelf"][0][1]["formedAt"]["timeDomainRef"],
        "time:real"
    );
    assert_eq!(
        actual_proposal["payload"]["output"],
        proposal["payload"]["output"]
    );
    assert_eq!(
        actual_outcome["payload"]["result"]["actions"][0]["selfRecord"]["formedAt"]["recordedAt"],
        recorded
    );
    assert_eq!(
        actual_proposal["payload"]["formedAt"]["recordedAt"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<Utc>>()
            .unwrap(),
        recorded.parse::<chrono::DateTime<Utc>>().unwrap()
    );
}

#[tokio::test]
async fn owner_state_window_uses_committed_world_coordinates_and_real_recording() {
    use nous_motivation::{Criterion, CriterionReader, OfficialCriterionReader, Truth};
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let world = runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Configure {
                time_domain_ref: "time:course".into(),
                rate: 0.0,
                world_time: Some(Utc::now() + Duration::days(4)),
            },
        )
        .await
        .unwrap();
    let source =
        test_support::observation(&runtime, subject, "a complete owner-state timeline").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Owner state in a paused course world",
        ))
        .await
        .unwrap();
    runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Advance { seconds: 3600.0 },
        )
        .await
        .unwrap();
    runtime
        .require_memory()
        .unwrap()
        .mutate_memory_lifecycle(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            memory.object.object_epoch,
            nous_memory::LifecycleAction::Suppress,
        )
        .await
        .unwrap();
    let end = runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Advance { seconds: 3600.0 },
        )
        .await
        .unwrap();
    let reader = OfficialCriterionReader {
        material: runtime.material.clone(),
        social: None,
    };
    let criterion = Criterion::OwnerState {
        version: 1,
        reference: CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
        state: "normal".into(),
    };
    let mut tx = runtime.store.read_view().await.unwrap();
    let read = reader
        .read_in(
            &mut tx,
            subject,
            &criterion,
            &DomainTemporalExtent {
                time_domain_ref: "time:course".into(),
                extent: TemporalExtent::Interval {
                    start: Some(world.world_now),
                    end: Some(end.world_now),
                },
            },
            &[],
        )
        .await
        .unwrap();
    assert!(
        read.complete,
        "actual complete owner history can close this world window"
    );
    assert_eq!(read.samples.first().unwrap().truth, Truth::True);
    assert_eq!(read.samples.last().unwrap().truth, Truth::False);
    assert_eq!(
        read.samples.last().unwrap().at,
        world.world_now + Duration::hours(1)
    );
    let other = reader
        .read_in(
            &mut tx,
            subject,
            &criterion,
            &DomainTemporalExtent {
                time_domain_ref: "time:other".into(),
                extent: TemporalExtent::Interval {
                    start: Some(world.world_now),
                    end: Some(end.world_now),
                },
            },
            &[],
        )
        .await
        .unwrap();
    assert!(!other.complete);
    assert!(other.samples.is_empty());
    tx.commit().await.unwrap();
    let recorded: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT max(recorded_at) FROM authority_object_states WHERE subject_id=$1 AND object_kind='memory'")
        .bind(subject.0).fetch_one(runtime.store.pool()).await.unwrap();
    assert!((recorded - Utc::now()).num_seconds().abs() < 2);
}

#[tokio::test]
async fn paused_world_idle_waits_for_semantic_advance_and_wakes_on_real_scheduler_time() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let world = runtime
        .change_clock(
            subject,
            OperationId::new(),
            ClockChange::Configure {
                time_domain_ref: "time:course".into(),
                rate: 0.0,
                world_time: Some(Utc::now() + Duration::days(7)),
            },
        )
        .await
        .unwrap();
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({}))
        .await
        .unwrap();
    let mut input = test_support::longitudinal::observation(subject, Some(session.session_id));
    input.occurrence.observed_at = None;
    let source = runtime.material.record_observation(input).await.unwrap();
    assert_eq!(
        source.occurrence.observed_stamp.time_domain_ref,
        "time:course"
    );
    assert!(
        (source.occurrence.observed_at - Utc::now())
            .num_seconds()
            .abs()
            < 2
    );
    let first = runtime
        .cognition
        .segment_experience(subject, "interaction", 128, false)
        .await
        .unwrap();
    assert!(first.ready.is_empty());
    let wake = first.next_due.unwrap();
    assert!(
        wake < world.world_now,
        "a scheduler deadline is real UTC, not a seven-day world coordinate"
    );
    let pending_before: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT due_at FROM maintenance_needs WHERE subject_id=$1 AND kind='episode_segment' AND state='pending'")
        .bind(subject.0).fetch_one(runtime.store.pool()).await.unwrap();
    let seconds = runtime
        .configuration
        .snapshot_for_subject(subject)
        .unwrap()
        .get(nous_configuration::ConfigKey::<u64>::new(
            "episode.hard_idle_seconds",
        ))
        .unwrap();
    runtime
        .change_clock(
            subject,
            OperationId::new(),
            ClockChange::Advance {
                seconds: seconds as f64 + 1.0,
            },
        )
        .await
        .unwrap();
    let pending_after: chrono::DateTime<Utc> = sqlx::query_scalar("SELECT due_at FROM maintenance_needs WHERE subject_id=$1 AND kind='episode_segment' AND state='pending'")
        .bind(subject.0).fetch_one(runtime.store.pool()).await.unwrap();
    assert!(pending_after <= pending_before);
    let matured = runtime
        .cognition
        .segment_experience(subject, "interaction", 128, false)
        .await
        .unwrap();
    assert_eq!(matured.ready.len(), 1);
    assert_eq!(
        matured.ready[0].boundary_reason.as_deref(),
        Some("hard_idle")
    );
    assert!(
        (matured.ready[0].observed_end - Utc::now())
            .num_seconds()
            .abs()
            < 2
    );
}
