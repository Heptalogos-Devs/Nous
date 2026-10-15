// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use chrono::{Duration, Utc};
use nous_core::{CognitiveRef, EntityRef, OperationId};
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, ResolvedEntityMention,
    RuntimeDirective,
};
use nous_runtime::{ResidentAdmission, ResidentState};
use test_support::{database, observation, open_runtime};
use uuid::Uuid;

async fn admit(
    runtime: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
    session: nous_core::SessionId,
    admissions: Vec<ResidentAdmission>,
) -> nous_core::Result<nous_runtime::ResidentMutationOutcome> {
    let mut tx = runtime.store.begin().await?;
    let result = runtime
        .cognition
        .admit_batch_in(&mut tx, subject, session, admissions)
        .await?;
    tx.commit()
        .await
        .map_err(nous_persistence::database_error)?;
    Ok(result)
}

#[tokio::test]
async fn observation_admission_is_a_single_idempotent_runtime_batch() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({"consumer":"a"}))
        .await
        .expect("session");
    let entity = EntityRef::new("entity:alice").expect("entity");
    let request_id = Uuid::new_v4();
    let input = ObservationInput {
        subject,
        session: Some(session.session_id),
        occurrence: OccurrenceDescriptor {
            source_class: nous_core::SourceClass::Message,
            external_object_ref: None,
            occurred_time: nous_core::TemporalExtent::Unknown,
            observed_at: Some(Utc::now()),
            conversation_ref: None,
            actor_entity_ref: None,
            context: serde_json::json!({}),
        },
        material: ObservationMaterial::InlineText {
            text: "Alice was present".into(),
            media_type: "text/plain".into(),
        },
        entities: vec![ResolvedEntityMention {
            surface: "Alice".into(),
            entity_ref: Some(entity.clone()),
            semantic_role: Some("subject".into()),
        }],
        runtime: RuntimeDirective::default(),
    };
    let first = runtime
        .material
        .record_observation_once(input.clone(), Some(request_id))
        .await
        .expect("first observation");
    let after_first = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .expect("resident session");
    assert_eq!(after_first.runtime_revision, 1);
    assert_eq!(after_first.resident.len(), 2);
    assert!(
        after_first.resident.iter().any(
            |value| value.reference == CognitiveRef::Occurrence(first.occurrence.occurrence_id)
        )
    );
    assert!(
        after_first
            .resident
            .iter()
            .any(|value| value.reference == CognitiveRef::Entity(entity.clone()))
    );

    let retry = runtime
        .material
        .record_observation_once(input.clone(), Some(request_id))
        .await
        .expect("idempotent observation retry");
    let after_retry = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .expect("resident retry session");
    assert_eq!(
        retry.occurrence.occurrence_id,
        first.occurrence.occurrence_id
    );
    assert_eq!(after_retry.runtime_revision, after_first.runtime_revision);
    assert_eq!(after_retry.resident.len(), after_first.resident.len());
    let mut rejected = input.clone();
    rejected.entities.clear();
    rejected.material = ObservationMaterial::ResourceAvailability {
        resource: nous_core::ResourceRef::new("resource:test:missing").unwrap(),
    };
    let rejected_key = Uuid::new_v4();
    assert!(matches!(
        runtime
            .material
            .record_observation_once(rejected, Some(rejected_key))
            .await,
        Err(nous_core::Error::FailedPrecondition(_))
    ));
    let occurrences: i64 =
        sqlx::query_scalar("SELECT count(*) FROM observation_occurrences WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    assert_eq!(
        occurrences, 1,
        "failed Runtime admission must roll back Material Authority"
    );
    let closed = runtime
        .cognition
        .close_session(subject, session.session_id)
        .await
        .unwrap();
    let replay = runtime
        .material
        .record_observation_once(input, Some(request_id))
        .await
        .unwrap();
    assert_eq!(
        replay.occurrence.occurrence_id,
        first.occurrence.occurrence_id
    );
    let after_closed_replay = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .unwrap();
    assert_eq!(
        after_closed_replay.runtime_revision,
        closed.runtime_revision
    );
}

#[tokio::test]
async fn resident_batch_is_atomic_and_invalid_persisted_refs_are_visible() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject_a = test_support::query::subject(&runtime).await;
    let subject_b = test_support::query::subject(&runtime).await;
    let session = runtime
        .cognition
        .open_session(subject_a, None, serde_json::json!({}))
        .await
        .expect("session");
    let foreign = observation(&runtime, subject_b, "foreign").await;
    let result = admit(
        &runtime,
        subject_a,
        session.session_id,
        vec![ResidentAdmission {
            last_meaningful_use_at: None,
            reference: CognitiveRef::Occurrence(foreign.occurrence.occurrence_id),
            reason: "foreign".into(),
            hold_until: None,
            state: ResidentState::Resident,
        }],
    )
    .await;
    assert!(matches!(
        result,
        Err(nous_core::Error::FailedPrecondition(_))
    ));
    let clean = runtime
        .cognition
        .session(subject_a, session.session_id)
        .await
        .expect("clean session");
    assert_eq!(clean.runtime_revision, 0);
    assert!(clean.resident.is_empty());

    sqlx::query("INSERT INTO resident_refs(session_id,ref_kind,ref_value,entered_at,entry_reason,state,metadata) VALUES($1,'unknown_ref','value',$2,'corrupt','resident','{}')")
        .bind(session.session_id.0)
        .bind(Utc::now())
        .execute(runtime.store.pool())
        .await
        .expect("persist invalid ref");
    let result = runtime
        .cognition
        .session(subject_a, session.session_id)
        .await;
    assert!(matches!(result, Err(nous_core::Error::Invalid(_))));
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one public residency scenario verifies shared admission policy, idempotency and concurrent close"
)]
async fn resident_noop_preserves_hold_and_revision_and_eviction_is_one_revision() {
    let (root, url, _postgres) = database().await;
    let mut runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({}))
        .await
        .expect("session");
    let first = observation(&runtime, subject, "first").await;
    let hold = Utc::now() + Duration::hours(1);
    let first_admission = ResidentAdmission {
        last_meaningful_use_at: None,
        reference: CognitiveRef::Occurrence(first.occurrence.occurrence_id),
        reason: "observed".into(),
        hold_until: Some(hold),
        state: ResidentState::Resident,
    };
    let inserted = admit(
        &runtime,
        subject,
        session.session_id,
        vec![first_admission.clone()],
    )
    .await
    .expect("first admission");
    assert!(inserted.changed);
    assert_eq!(inserted.runtime_revision, 1);
    let no_op = admit(
        &runtime,
        subject,
        session.session_id,
        vec![ResidentAdmission {
            last_meaningful_use_at: None,
            hold_until: Some(Utc::now()),
            ..first_admission
        }],
    )
    .await
    .expect("no-op admission");
    assert!(!no_op.changed);
    assert_eq!(no_op.runtime_revision, 1);
    let resident = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .expect("resident after no-op")
        .resident;
    assert_eq!(
        resident[0].hold_until.map(|value| value.timestamp_micros()),
        Some(hold.timestamp_micros())
    );

    runtime.cognition.resident_limit = 1;
    let second = observation(&runtime, subject, "second").await;
    let eviction = admit(
        &runtime,
        subject,
        session.session_id,
        vec![ResidentAdmission {
            last_meaningful_use_at: None,
            reference: CognitiveRef::Occurrence(second.occurrence.occurrence_id),
            reason: "observed".into(),
            hold_until: None,
            state: ResidentState::Resident,
        }],
    )
    .await
    .expect("eviction admission");
    assert_eq!(eviction.evicted_count, 1);
    assert_eq!(eviction.runtime_revision, 2);
    assert_eq!(
        runtime
            .cognition
            .session(subject, session.session_id)
            .await
            .expect("evicted session")
            .resident
            .len(),
        1
    );
    let mut events = Vec::new();
    for occurrence in [
        first.occurrence.occurrence_id,
        second.occurrence.occurrence_id,
    ] {
        let memory = runtime
            .require_memory()
            .unwrap()
            .form_memory(test_support::form_input(
                subject,
                occurrence,
                OperationId::new(),
                "bounded use resident",
            ))
            .await
            .unwrap();
        events.push(nous_runtime::UseFeedbackEvent {
            query_id: None,
            event_id: nous_core::UseEventId::new(),
            reference: CognitiveRef::MemoryRevision(memory.revision.memory_revision_id),
            use_kind: nous_runtime::UseKind::Referenced,
            occurred_at: runtime.cognition.now(subject),
            context: serde_json::json!({}),
        });
    }
    let uses = nous_runtime::UseFeedback {
        model_input: false,
        subject,
        session_id: Some(session.session_id),
        consumer_ref: "consumer:resident-test".into(),
        events,
    };
    assert_eq!(
        runtime.cognition.use_feedback(uses.clone()).await.unwrap(),
        (2, 0, Some(3))
    );
    assert_eq!(
        runtime.cognition.use_feedback(uses).await.unwrap(),
        (0, 2, Some(3))
    );
    let after_use = runtime
        .cognition
        .session(subject, session.session_id)
        .await
        .unwrap();
    assert_eq!(after_use.resident.len(), 1);
    assert_eq!(
        after_use.resident[0].reference,
        CognitiveRef::Occurrence(first.occurrence.occurrence_id)
    );
    assert_eq!(
        after_use.resident[0]
            .hold_until
            .map(|at| at.timestamp_micros()),
        Some(hold.timestamp_micros())
    );
    let (closed_a, closed_b) = tokio::join!(
        runtime.cognition.close_session(subject, session.session_id),
        runtime.cognition.close_session(subject, session.session_id)
    );
    assert_eq!(
        usize::from(closed_a.is_ok()) + usize::from(closed_b.is_ok()),
        1
    );
    assert_eq!(
        runtime
            .cognition
            .session(subject, session.session_id)
            .await
            .unwrap()
            .runtime_revision,
        4
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one public projection scenario covers policy, budget and distinct lifecycle refusals"
)]
async fn projection_materializes_supported_memory_within_the_text_budget() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let source = observation(&runtime, subject, "Grounded projection content").await;
    let memory = runtime
        .require_memory()
        .expect("memory")
        .form_memory(test_support::form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Grounded projection content",
        ))
        .await
        .expect("formed memory");
    let owner = runtime.require_memory().unwrap();
    let reference = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
    let policy = nous_memory::resolve_accessibility_policy(
        &runtime.configuration.snapshot_for_subject(subject).unwrap(),
    )
    .unwrap();
    let mut sources = owner
        .context_sources(
            subject,
            &[(reference.clone(), false), (reference.clone(), true)],
            &policy,
            runtime.cognition.now(subject) + Duration::days(36_500),
            8,
        )
        .await
        .unwrap()
        .into_iter();
    let aged = sources
        .next()
        .unwrap()
        .err()
        .expect("unrequested old memory must require explicit recall");
    assert!(aged.to_string().contains("accessibility"));
    assert!(matches!(
        sources.next().unwrap(),
        Err(nous_core::Error::Unavailable(_))
    ));
    let permissive_policy = nous_memory::AccessibilityPolicy {
        normal_threshold: -100.0,
        deep_threshold: -101.0,
        ..policy
    };
    owner
        .context_sources(
            subject,
            &[(reference.clone(), false)],
            &permissive_policy,
            runtime.cognition.now(subject) + Duration::days(36_500),
            256,
        )
        .await
        .unwrap()
        .pop()
        .unwrap()
        .expect("caller policy controls accessibility");
    let session = runtime
        .cognition
        .open_session(subject, None, serde_json::json!({}))
        .await
        .expect("session");
    let service = nous_kernel::transport::KernelService(runtime);
    let request = nous_protocol::public::ProjectionRequest {
        subject_id: subject.0.to_string(),
        session_id: session.session_id.0.to_string(),
        consumer_id: "default".into(),
        max_items: 4,
        max_text_bytes: 8,
        memory_requirement: "REQUIRED".into(),
        runtime_requirement: "OPTIONAL".into(),
        resource_requirement: "OPTIONAL".into(),
        materialize: true,
        situation_refs: vec![nous_protocol::public::CognitiveRef {
            kind: "memory_revision".into(),
            value: memory.revision.memory_revision_id.0.to_string(),
        }],
        ..Default::default()
    };
    let omitted = service
        .build_contribution_batch(request.clone())
        .await
        .unwrap();
    assert!(omitted.segments.is_empty());
    assert!(
        omitted.degradation[0]
            .detail
            .contains("complete cognitive unit")
    );
    let request = nous_protocol::public::ProjectionRequest {
        max_text_bytes: 256,
        ..request
    };
    let projection = service
        .build_contribution_batch(request.clone())
        .await
        .expect("projection");
    assert!(projection.degradation.is_empty());
    assert_eq!(projection.segments[0].text, "Grounded projection content");
    assert_eq!(projection.segments[0].evidence.len(), 1);
    assert_eq!(
        projection.segments[0].source_revision.as_deref(),
        Some(memory.revision.memory_revision_id.0.to_string().as_str())
    );
    assert_eq!(projection.subject_id, subject.0.to_string());
    assert_eq!(projection.session_id, session.session_id.0.to_string());
    let mut material_request = request.clone();
    material_request.memory_requirement = "FORBIDDEN".into();
    material_request.runtime_requirement = "REQUIRED".into();
    material_request.max_items = 1;
    material_request.situation_refs = vec![nous_protocol::public::CognitiveRef {
        kind: "occurrence".into(),
        value: source.occurrence.occurrence_id.0.to_string(),
    }];
    let material = service
        .build_contribution_batch(material_request)
        .await
        .unwrap();
    assert_eq!(material.segments.len(), 1);
    assert_eq!(material.segments[0].text, "Grounded projection content");
    assert_eq!(material.segments[0].authority, "evidence");
    let owner = service.0.require_memory().unwrap();
    owner
        .mutate_memory_lifecycle(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            1,
            nous_memory::LifecycleAction::Suppress,
        )
        .await
        .unwrap();
    let suppressed = service
        .build_contribution_batch(request.clone())
        .await
        .unwrap();
    assert!(suppressed.segments.is_empty());
    assert!(
        suppressed.degradation[0]
            .detail
            .contains("Memory is suppressed")
    );
    owner
        .mutate_memory_lifecycle(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            2,
            nous_memory::LifecycleAction::Restore,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE memory_objects SET integrity_state='revalidation_required' WHERE memory_id=$1",
    )
    .bind(memory.object.memory_id.0)
    .execute(service.0.store.pool())
    .await
    .unwrap();
    let invalidated = service
        .build_contribution_batch(request.clone())
        .await
        .unwrap();
    assert!(invalidated.segments.is_empty());
    assert!(
        invalidated.degradation[0]
            .detail
            .contains("Memory requires revalidation")
    );
    sqlx::query("UPDATE memory_objects SET integrity_state='valid' WHERE memory_id=$1")
        .bind(memory.object.memory_id.0)
        .execute(service.0.store.pool())
        .await
        .unwrap();
    owner
        .mutate_memory_lifecycle(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            3,
            nous_memory::LifecycleAction::Withdraw,
        )
        .await
        .unwrap();
    let withdrawn = service
        .build_contribution_batch(request.clone())
        .await
        .unwrap();
    assert!(withdrawn.segments.is_empty());
    assert!(
        withdrawn.degradation[0]
            .detail
            .contains("Memory is withdrawn")
    );
    owner
        .mutate_memory_lifecycle(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            4,
            nous_memory::LifecycleAction::Reaccept,
        )
        .await
        .unwrap();
    owner
        .purge_memory(subject, memory.object.memory_id, OperationId::new(), 5)
        .await
        .unwrap();
    let purged = service.build_contribution_batch(request).await.unwrap();
    assert!(purged.segments.is_empty());
    assert_eq!(purged.degradation[0].code, "projection_source_unavailable");
    service
        .0
        .cognition
        .close_session(subject, session.session_id)
        .await
        .unwrap();
    let closed = nous_protocol::public::ProjectionRequest {
        subject_id: subject.0.to_string(),
        session_id: session.session_id.0.to_string(),
        consumer_id: "default".into(),
        max_items: 1,
        max_text_bytes: 8,
        memory_requirement: "OPTIONAL".into(),
        runtime_requirement: "OPTIONAL".into(),
        resource_requirement: "OPTIONAL".into(),
        materialize: true,
        ..Default::default()
    };
    assert!(matches!(service.build_contribution_batch(closed).await,
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::StaleContext));
}
