// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;
use nous_core::*;
use nous_memory::CreateTagRequest;
use test_support::*;

fn query(subject: SubjectId) -> CognitiveQuery {
    CognitiveQuery {
        subject,
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "Alice develops Nous Wave".into(),
            })],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one owner scenario preserves WorkContext mutation and query snapshot ordering"
)]
async fn preparation_captures_work_context_exact_sources_and_frozen_policy_without_serving() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let entity = EntityRef::new("entity:alice-opaque").unwrap();
    runtime
        .store
        .bind_identity(
            subject,
            CognitiveRef::Entity(entity.clone()),
            "Alice".into(),
            vec!["A".into()],
        )
        .await
        .unwrap();
    let observed = observation(&runtime, subject, "Alice plans the Nous Wave release").await;
    let mut input = form_input(
        subject,
        observed.occurrence.occurrence_id,
        OperationId::new(),
        "Alice plans the Nous Wave release",
    );
    input.content.aboutness = vec![entity.clone()];
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(input)
        .await
        .unwrap();
    let reference = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    let session = runtime
        .cognition
        .open_session(
            subject,
            None,
            serde_json::json!({"transcript":"UNRELATED HISTORY MUST NOT BE EMBEDDED"}),
        )
        .await
        .unwrap();
    let pinned_tag = runtime
        .require_memory()
        .unwrap()
        .create_tag(
            subject,
            CreateTagRequest {
                producer: None,
                operation_id: OperationId::new(),
                content: nous_memory::TagContent {
                    label: "Release planning".into(),
                    description: Some("Subject release checkpoint".into()),
                    kind_hint: None,
                },
                origin: "host_explicit".into(),
            },
        )
        .await
        .unwrap()
        .tag_id;
    let context = runtime
        .cognition
        .create_work_context(nous_runtime::CreateWorkContextInput {
            operation_id: OperationId::new(),
            subject,
            purpose: "Nous Wave release preparation".into(),
            unresolved_questions: vec!["Alice's pending release tasks".into()],
            constraints: serde_json::json!({"review_rule":"Do not deploy without Alice approval"}),
            resume_conditions: vec!["Continue when Alice has reviewed the release".into()],
            budget_summary: serde_json::json!({"task_effort":"One release review remains"}),
            context_text: "TASK CONTEXT ".repeat(150),
            entity_anchors: vec![entity.clone()],
            tag_anchors: vec![pinned_tag],
            cognition_anchors: vec![reference.clone()],
        })
        .await
        .unwrap();
    runtime
        .cognition
        .set_active_work_context(
            subject,
            session.session_id,
            Some(context.work_context_id),
            session.runtime_revision,
            OperationId::new(),
        )
        .await
        .unwrap();
    let (page, more) = runtime
        .cognition
        .list_work_contexts(subject, 1, None)
        .await
        .unwrap();
    assert!(!more);
    assert_eq!(page[0].work_context_id, context.work_context_id);
    assert_eq!(page[0].cognition_anchors, context.cognition_anchors);
    assert_eq!(page[0].entity_anchors, context.entity_anchors);
    assert_eq!(page[0].tag_anchors, context.tag_anchors);
    let projection = nous_kernel::transport::KernelService(runtime.clone())
        .build_contribution_batch(nous_protocol::public::ProjectionRequest {
            subject_id: subject.0.to_string(),
            session_id: session.session_id.0.to_string(),
            consumer_id: "default".into(),
            max_items: 1,
            max_text_bytes: 256,
            memory_requirement: "REQUIRED".into(),
            runtime_requirement: "FORBIDDEN".into(),
            resource_requirement: "FORBIDDEN".into(),
            materialize: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        projection.active_work_context_id.as_deref(),
        Some(context.work_context_id.to_string().as_str())
    );
    assert_eq!(projection.segments.len(), 1);
    assert!(!projection.segments[0].text.is_empty());
    let service = nous_kernel::transport::KernelService(runtime.clone());
    let foreground_request = nous_protocol::public::ProjectionRequest {
        subject_id: subject.0.to_string(),
        session_id: session.session_id.0.to_string(),
        consumer_id: "default".into(),
        max_items: 1,
        max_text_bytes: 96,
        memory_requirement: "OPTIONAL".into(),
        runtime_requirement: "REQUIRED".into(),
        resource_requirement: "FORBIDDEN".into(),
        materialize: true,
        ..Default::default()
    };
    assert!(matches!(
        service
            .build_contribution_batch(foreground_request.clone())
            .await,
        Err(Error::Unavailable(_))
    ));
    let foreground_request = nous_protocol::public::ProjectionRequest {
        max_text_bytes: 4096,
        ..foreground_request
    };
    let foreground = service
        .build_contribution_batch(foreground_request.clone())
        .await
        .unwrap();
    assert_eq!(foreground.segments.len(), 1);
    assert_eq!(foreground.segments[0].source_refs[0].kind, "work_context");
    assert_eq!(
        foreground.segments[0].source_refs[0].value,
        context.work_context_id.to_string()
    );
    assert!(
        foreground.segments[0]
            .text
            .contains("Nous Wave release preparation")
    );
    assert!(
        foreground.segments[0]
            .text
            .contains("Alice's pending release tasks")
    );
    assert!(foreground.segments[0].text.contains("TASK CONTEXT"));
    assert!(foreground.segments[0].text.len() > 96);
    assert_eq!(
        foreground.segments[0].authority,
        "external_current_authority"
    );
    let mut entity_request = foreground_request;
    entity_request.max_items = 2;
    entity_request.max_text_bytes = 4096;
    let with_entity = service
        .build_contribution_batch(entity_request)
        .await
        .unwrap();
    assert!(
        with_entity.segments[0]
            .text
            .contains("Do not deploy without Alice approval")
    );
    assert!(
        with_entity.segments[0]
            .text
            .contains("One release review remains")
    );
    assert!(
        with_entity.segments[0]
            .text
            .contains("Continue when Alice has reviewed the release")
    );
    let entity_source = with_entity
        .segments
        .iter()
        .find(|segment| segment.source_refs[0].kind == "entity")
        .unwrap();
    assert_eq!(entity_source.text, "Alice (A)");
    assert_eq!(entity_source.authority, "external_current_authority");
    let mut request = query(subject);
    request.session = Some(session.session_id);
    request
        .expression
        .cues
        .push(Cue::Entity(EntityCue { entity_ref: entity }));
    let first = runtime
        .cognition
        .bind_query(request.clone(), &runtime)
        .await
        .unwrap();
    assert!(
        first
            .representation
            .text
            .contains("Nous Wave release preparation")
    );
    assert!(first.representation.text.contains("Alice (A)"));
    assert!(
        first
            .representation
            .text
            .contains("Do not deploy without Alice approval")
    );
    assert!(
        first
            .representation
            .text
            .contains("One release review remains")
    );
    assert!(
        first
            .representation
            .text
            .contains("Continue when Alice has reviewed the release")
    );
    assert!(
        first
            .representation
            .text
            .contains("Alice plans the Nous Wave release")
    );
    assert!(!first.representation.text.contains("UNRELATED HISTORY"));
    assert!(!first.representation.text.contains("entity:alice-opaque"));
    assert!(
        first
            .runtime_sources()
            .any(|source| source.reference == reference)
    );
    assert_eq!(
        first
            .context_snapshot
            .work_context
            .as_ref()
            .unwrap()
            .entity_anchors,
        vec![EntityRef::new("entity:alice-opaque").unwrap()]
    );
    assert!(first.representation.text.matches("TASK CONTEXT").count() > 100);
    for profile in nous_runtime::CognitiveProfile::ALL {
        let selected = first.for_profile(profile).unwrap();
        assert!(selected.enabled_lanes.contains(&EvidenceFamily::TagDirect));
        assert!(
            selected
                .activation
                .explicit_tags
                .iter()
                .any(|tag| tag.tag == pinned_tag
                    && tag.source == nous_runtime::ActivationSource::WorkContext)
        );
        assert_eq!(
            selected.context_snapshot.source_refs,
            first.context_snapshot.source_refs
        );
        assert_eq!(selected.representation.sha256, first.representation.sha256);
    }
    assert!(
        first
            .context_snapshot
            .source_refs
            .iter()
            .any(|source| source.reference == reference && source.origin == "work_context")
    );
    assert!(
        runtime
            .store
            .serving_current(subject)
            .await
            .unwrap()
            .is_empty()
    );
    let selected = first
        .for_profile(nous_runtime::CognitiveProfile::VcpDtsc)
        .unwrap();
    assert_eq!(selected.representation.sha256, first.representation.sha256);
    assert_eq!(
        selected.context_snapshot.source_refs,
        first.context_snapshot.source_refs
    );
    assert_eq!(
        selected
            .config_snapshot
            .source(nous_runtime::COGNITIVE_PROFILE.path()),
        Some(nous_configuration::ConfigSource::OperationOverride)
    );
    assert_eq!(
        runtime
            .configuration
            .snapshot_for_subject(subject)
            .unwrap()
            .effective_digest,
        first.config_snapshot.effective_digest
    );
    assert!(
        first
            .config_snapshot
            .query_override(nous_runtime::QUERY_SLOTS, 1)
            .is_err()
    );
    let token = runtime
        .cognition
        .retain_prepared_query(test_support::query_reservation(first.clone()))
        .unwrap();
    runtime
        .configuration
        .set_system_override(
            OperationId::new(),
            nous_runtime::COGNITIVE_PROFILE.path(),
            serde_json::json!("baseline-rrf"),
            None,
        )
        .await
        .unwrap();
    let frozen = runtime
        .cognition
        .take_prepared_query(subject, token)
        .unwrap();
    let timed = runtime
        .cognition
        .bind_query(query(subject), &runtime)
        .await
        .unwrap();
    let timed_token = runtime
        .cognition
        .retain_prepared_query(
            nous_runtime::QueryReservation::new(
                timed,
                nous_runtime::QueryLease::new(std::time::Duration::from_secs(2)).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1400)).await;
    let resumed = runtime
        .cognition
        .take_prepared_query(subject, timed_token)
        .unwrap();
    let next_token = runtime.cognition.retain_prepared_query(resumed).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    assert!(
        runtime
            .cognition
            .prepared_query(subject, next_token)
            .is_err(),
        "activation must not renew the original execution opportunity"
    );
    assert_eq!(
        frozen.bound.config_snapshot.effective_digest,
        first.config_snapshot.effective_digest
    );
    assert_eq!(
        frozen.bound.retrieval_policy.cognitive_profile,
        first.retrieval_policy.cognitive_profile
    );
    assert!(
        runtime
            .cognition
            .take_prepared_query(subject, token)
            .is_err()
    );
    runtime
        .cognition
        .update_work_context(nous_runtime::UpdateWorkContextInput {
            operation_id: OperationId::new(),
            subject,
            work_context_id: context.work_context_id,
            expected_revision: context.revision,
            intent: nous_runtime::WorkContextIntent::Fields(nous_runtime::WorkContextFields {
                purpose: Some("Nous Wave release stabilization".into()),
                context_text: Some(String::new()),
                ..Default::default()
            }),
        })
        .await
        .unwrap();
    assert_eq!(
        first
            .context_snapshot
            .work_context
            .as_ref()
            .unwrap()
            .revision,
        1
    );
    assert!(
        first
            .representation
            .text
            .contains(&"TASK CONTEXT ".repeat(100))
    );
    assert_eq!(
        frozen.bound.context_snapshot.source_refs,
        first.context_snapshot.source_refs
    );
    let changed = runtime
        .cognition
        .bind_query(request, &runtime)
        .await
        .unwrap();
    assert_ne!(changed.representation.sha256, first.representation.sha256);
    let mut runtime_query = query(subject);
    runtime_query.session = Some(session.session_id);
    let frozen_runtime = runtime
        .cognition
        .bind_query(runtime_query, &runtime)
        .await
        .unwrap();
    let newer_source = observation(&runtime, subject, "Bob reports an unrelated task").await;
    let newer = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            newer_source.occurrence.occurrence_id,
            OperationId::new(),
            "Bob reports an unrelated task",
        ))
        .await
        .unwrap();
    let newer_ref = CognitiveRef::MemoryRevision(newer.revision.memory_revision_id);
    runtime
        .cognition
        .use_feedback(nous_runtime::UseFeedback {
            model_input: false,
            subject,
            session_id: Some(session.session_id),
            consumer_ref: "consumer:test:prepared-query".into(),
            events: vec![nous_runtime::UseFeedbackEvent {
                query_id: None,
                event_id: UseEventId::new(),
                reference: newer_ref.clone(),
                use_kind: nous_runtime::UseKind::Referenced,
                occurred_at: chrono::Utc::now(),
                context: serde_json::json!({}),
            }],
        })
        .await
        .unwrap();
    let frozen_results = Box::pin(runtime.execute_bound_query(frozen_runtime, None))
        .await
        .unwrap()
        .result;
    assert!(
        frozen_results
            .results
            .iter()
            .any(|hit| hit.reference == reference)
    );
    assert!(
        !frozen_results
            .results
            .iter()
            .any(|hit| hit.reference == newer_ref)
    );
    let mut raw = query(subject);
    raw.expression.cues = vec![Cue::Text(TextCue {
        text: "What did I discuss yesterday?".into(),
    })];
    assert!(
        runtime
            .cognition
            .bind_query(raw.clone(), &runtime)
            .await
            .unwrap()
            .representation
            .text
            .contains("What did I discuss yesterday?")
    );
    raw.session = Some(session.session_id);
    assert!(runtime.cognition.bind_query(raw, &runtime).await.is_ok());
}
