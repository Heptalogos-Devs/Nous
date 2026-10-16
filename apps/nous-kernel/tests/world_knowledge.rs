// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{Duration, SubsecRound, Utc};
use nous_core::*;
use nous_material::*;
use test_support::*;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real PostgreSQL journey proves late world contacts, independent history cuts, old revisions, knowledge contribution and clock restart"
)]
async fn late_realization_has_story_knowledge_without_backdating_real_history() {
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
                motivation: true,
            }),
            cognitive_seed: None,
        })
        .await
        .unwrap()
        .subject_id;
    let real_before = Utc::now();
    let story_seen = (real_before - Duration::hours(10)).trunc_subsecs(6);
    let world_cut = story_seen + Duration::hours(1);
    runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            nous_runtime::ClockChange::Configure {
                time_domain_ref: "time:school".into(),
                rate: 0.0,
                world_time: Some(Utc::now() - Duration::hours(1)),
            },
        )
        .await
        .unwrap();
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({}))
        .await
        .unwrap();
    let input = ObservationInput {
        subject,
        session: Some(session.session_id),
        occurrence: OccurrenceDescriptor {
            source_class: SourceClass::WorldRealized,
            world_realized: Some(WorldRealizedSource {
                world_id: "world:school".into(),
                event_id: "event:late-class".into(),
                event_revision: "1".into(),
                producer_ref: "rule:school-timetable".into(),
                generation_method: "rule".into(),
                realized_at: Utc::now(),
                world_observed: DomainStamp {
                    time_domain_ref: "time:school".into(),
                    at: story_seen,
                    recorded_at: Utc::now(),
                },
                shared_origin_refs: vec!["realization:one".into()],
            }),
            external_object_ref: None,
            occurred_time: TemporalExtent::Instant { at: story_seen },
            occurred_time_domain_ref: "time:school".into(),
            observed_at: None,
            observed_stamp: None,
            conversation_ref: None,
            actor_entity_ref: Some(EntityRef::new("entity:school:teacher").unwrap()),
            context: serde_json::json!({}),
        },
        material: ObservationMaterial::InlineText {
            text: "Teacher announced the exam.".into(),
            media_type: "text/plain".into(),
        },
        entities: vec![],
        runtime: Default::default(),
    };
    let source = runtime
        .material
        .record_observation_once(input.clone(), Some(uuid::Uuid::new_v4()))
        .await
        .unwrap();
    assert!(source.occurrence.observed_at >= real_before);
    assert_eq!(source.occurrence.observed_stamp.at, story_seen);
    assert!(source.occurrence.world_realized.is_some());
    let occurrence = CognitiveRef::Occurrence(source.occurrence.occurrence_id);
    let query = CognitiveQuery {
        subject,
        temporal_frame: TemporalFrame {
            knowledge_at_world_time: Some(Box::new(DomainStamp {
                time_domain_ref: "time:school".into(),
                at: world_cut,
                recorded_at: Utc::now(),
            })),
            ..Default::default()
        },
        projection: ResultProjection {
            domains: vec![ResultDomain::Evidence],
        },
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "What did I hear before the school cutoff?".into(),
            })],
            targets: vec![QueryTarget::Exact {
                reference: occurrence.clone(),
            }],
            ..Default::default()
        },
        ..test_support::query::query(subject)
    };
    let result = runtime.execute_query(query.clone(), None).await.unwrap();
    assert_eq!(result.result.results.len(), 1);
    assert_eq!(result.result.results[0].reference, occurrence);
    let mut recorded_before = query.clone();
    recorded_before.temporal_frame.authority_view = AuthorityView::AsOf(real_before);
    assert!(runtime.execute_query(recorded_before, None).await.is_err());
    let mut earlier = query;
    earlier
        .temporal_frame
        .knowledge_at_world_time
        .as_mut()
        .unwrap()
        .at = story_seen - Duration::seconds(1);
    assert!(runtime.execute_query(earlier, None).await.is_err());
    let episodes = runtime
        .organize_experience(subject, 128, true)
        .await
        .unwrap();
    assert_eq!(episodes.episodes.len(), 1);
    assert_eq!(
        episodes.episodes[0].revision.experience_time_domain_ref,
        "time:school"
    );
    assert!(
        matches!(episodes.episodes[0].revision.experience_time, TemporalExtent::Instant { at } if at == story_seen)
    );
    let episode_scope = episodes.episodes[0]
        .revision
        .episode_revision_id
        .0
        .to_string();
    let waiting = runtime
        .require_memory()
        .unwrap()
        .plan_consolidation_scope(subject, "episode_revision", &episode_scope)
        .await
        .unwrap();
    assert_eq!(
        waiting.status, "deferred",
        "old story occurrence time cannot mature a newly committed paused-world Episode"
    );

    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "The original exam understanding",
        ))
        .await
        .unwrap();
    let formation = memory.revision.formed_at;
    runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            nous_runtime::ClockChange::Advance { seconds: 7200.0 },
        )
        .await
        .unwrap();
    let ready = runtime
        .require_memory()
        .unwrap()
        .plan_consolidation_scope(subject, "episode_revision", &episode_scope)
        .await
        .unwrap();
    assert_eq!(ready.status, "ready");

    let mut revised_content = form_input(
        subject,
        source.occurrence.occurrence_id,
        OperationId::new(),
        "The revised exam understanding",
    )
    .content;
    revised_content.basis = memory.basis.clone();
    let revised = runtime
        .require_memory()
        .unwrap()
        .revise_memory(nous_memory::ReviseMemoryInput {
            operation_id: OperationId::new(),
            subject,
            memory_id: memory.object.memory_id,
            expected_object_epoch: memory.object.object_epoch,
            intent: nous_memory::RevisionIntent::Rephrase,
            content: revised_content,
        })
        .await
        .unwrap();
    let mut past_memory = test_support::query::query(subject);
    past_memory.expression.targets = vec![QueryTarget::Exact {
        reference: CognitiveRef::Memory(memory.object.memory_id),
    }];
    past_memory.temporal_frame.knowledge_at_world_time = Some(Box::new(DomainStamp {
        time_domain_ref: "time:school".into(),
        at: formation + Duration::hours(1),
        recorded_at: Utc::now(),
    }));
    let old = runtime.execute_query(past_memory, None).await.unwrap();
    assert_eq!(
        old.result.results[0].revision,
        Some(CognitiveRef::MemoryRevision(
            memory.revision.memory_revision_id
        ))
    );
    assert_ne!(
        old.result.results[0].revision,
        Some(CognitiveRef::MemoryRevision(
            revised.revision.memory_revision_id
        ))
    );
    let knowledge = runtime.require_self().unwrap().adopt(nous_self::AdoptSelf {
        subject, operation_id: OperationId::new(), reason: "I heard the actual school announcement".into(),
        content: nous_self::SelfContent::SelfFacet(nous_self::SelfFacetContent {
            kind: "domain_knowledge".into(),
            domain_knowledge: Some(Box::new(DomainKnowledge {
                domain_ref: "topic:school-exams".into(), familiarity: "aware".into(),
                can_do: vec!["repeat the exam announcement".into()],
                limitations: vec!["have not practiced the exam questions".into()],
                experience_kinds: vec!["heard".into()], interest_refs: vec![],
            })),
            statement: "I heard the exam announcement; hearing it does not mean I practiced the questions".into(),
            epistemic_class: EpistemicClass::Observed, scope: nous_self::SelfScope {
                description: "this school's exam".into(), ..Default::default()
            }, stance: "adopted".into(), exceptions: vec![], basis: memory.basis.clone(),
            valid_time: TemporalExtent::Unknown, valid_time_domain_ref: "time:school".into(), producer: None,
        }),
    }).await.unwrap();
    let contribution = runtime
        .require_self()
        .unwrap()
        .context_source(subject, &knowledge.reference(), true, None)
        .await
        .unwrap();
    assert_eq!(contribution.domain_knowledge.unwrap().familiarity, "aware");
    assert_eq!(contribution.epistemic_class, Some(EpistemicClass::Observed));
    let contact_clock = runtime.clocks.read(subject).await.unwrap();
    let mut real_contact = input.clone();
    real_contact.occurrence.source_class = SourceClass::Message;
    real_contact.occurrence.world_realized = None;
    real_contact.occurrence.occurred_time_domain_ref = "time:real".into();
    real_contact.occurrence.occurred_time = TemporalExtent::Unknown;
    real_contact.occurrence.observed_at = Some(contact_clock.recorded_at);
    real_contact.occurrence.observed_stamp = Some(DomainStamp {
        time_domain_ref: contact_clock.time_domain_ref.clone(),
        at: contact_clock.world_now,
        recorded_at: contact_clock.recorded_at,
    });
    real_contact.occurrence.actor_entity_ref = Some(EntityRef::new("entity:qq:person").unwrap());
    let actual_qq = runtime
        .material
        .record_observation(real_contact.clone())
        .await
        .unwrap();
    assert_eq!(actual_qq.occurrence.observed_at, contact_clock.recorded_at);
    assert_eq!(
        actual_qq.occurrence.observed_stamp.at,
        contact_clock.world_now
    );
    assert_eq!(actual_qq.occurrence.source_class, SourceClass::Message);
    let mut known_qq = test_support::query::query(subject);
    known_qq.projection = ResultProjection {
        domains: vec![ResultDomain::Evidence],
    };
    known_qq.expression.targets = vec![QueryTarget::Exact {
        reference: CognitiveRef::Occurrence(actual_qq.occurrence.occurrence_id),
    }];
    known_qq.temporal_frame.knowledge_at_world_time = Some(Box::new(DomainStamp {
        time_domain_ref: "time:school".into(),
        at: contact_clock.world_now,
        recorded_at: Utc::now(),
    }));
    assert_eq!(
        runtime
            .execute_query(known_qq, None)
            .await
            .unwrap()
            .result
            .results
            .len(),
        1
    );
    real_contact.occurrence.observed_at = Some(real_before - Duration::days(1));
    assert!(
        runtime
            .material
            .record_observation(real_contact.clone())
            .await
            .is_err()
    );
    real_contact.occurrence.observed_stamp = None;
    let old_qq = runtime
        .material
        .record_observation(real_contact)
        .await
        .unwrap();
    assert_eq!(
        old_qq.occurrence.observed_stamp.time_domain_ref,
        "time:real"
    );
    let mut forged = input;
    forged.occurrence.world_realized = None;
    assert!(runtime.material.record_observation(forged).await.is_err());
    let expected_clock = runtime.clocks.read(subject).await.unwrap();
    let restored = open_runtime(&url, &root).await;
    let restored_clock = restored.clocks.read(subject).await.unwrap();
    assert_eq!(restored_clock.time_domain_ref, "time:school");
    assert_eq!(restored_clock.world_now, expected_clock.world_now);
}
