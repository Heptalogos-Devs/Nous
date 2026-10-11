// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use nous_core::*;
use nous_runtime::{
    CognitiveRuntimeService, ManualCognitiveClock, QUERY_FEEDBACK_RETENTION, UseFeedback,
    UseFeedbackEvent, UseKind,
};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use sqlx::Row;
use std::sync::Arc;
use test_support::*;
fn query(subject: SubjectId, reference: CognitiveRef) -> CognitiveQuery {
    CognitiveQuery {
        subject,
        session: None,
        work_context: None,
        projection: Default::default(),
        temporal_frame: Default::default(),

        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "Inspect this exact cognition".into(),
            })],
            targets: vec![QueryTarget::Exact { reference }],
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
async fn subject(rt: &nous_kernel::NousRuntime) -> SubjectId {
    rt.subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            },
            metadata: serde_json::json!({}),
            capabilities: None,
        })
        .await
        .unwrap()
        .subject_id
}
async fn memory(rt: &nous_kernel::NousRuntime, subject: SubjectId, text: &str) -> CognitiveRef {
    let occurrence = observation(rt, subject, text).await;
    let formed = rt
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            occurrence.occurrence.occurrence_id,
            OperationId::new(),
            text,
        ))
        .await
        .unwrap();
    CognitiveRef::MemoryRevision(formed.revision.memory_revision_id)
}
fn feedback(
    subject: SubjectId,
    reference: CognitiveRef,
    query_id: uuid::Uuid,
    now: chrono::DateTime<chrono::Utc>,
) -> UseFeedback {
    UseFeedback {
        subject,
        session_id: None,
        consumer_ref: "consumer:host:test".into(),
        events: vec![UseFeedbackEvent {
            query_id: Some(query_id),
            event_id: UseEventId::new(),
            reference,
            use_kind: UseKind::Referenced,
            occurred_at: now,
            context: serde_json::json!({}),
        }],
    }
}
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one owner scenario verifies final membership, Subject isolation, idempotency and expiry without persisting retrieved text"
)]
async fn linked_use_is_bounded_subject_local_final_and_expiring() {
    let (root, url, _pg) = database().await;
    let mut rt = open_runtime(&url, &root).await;
    let subject_a = subject(&rt).await;
    let subject_b = subject(&rt).await;
    let a = memory(
        &rt,
        subject_a,
        "PRIVATE retrieved source text must never enter query feedback",
    )
    .await;
    let b = memory(&rt, subject_a, "another unrelated cognition").await;
    let foreign = memory(&rt, subject_b, "other Subject cognition").await;
    let tag = rt
        .require_memory()
        .unwrap()
        .create_tag(
            subject_a,
            nous_memory::CreateTagRequest {
                operation_id: OperationId::new(),
                label: "Zeta opaque concept".into(),
                description: Some("No textual overlap with the focus".into()),
                kind_hint: None,
                origin: "host_explicit".into(),
                producer: None,
            },
        )
        .await
        .unwrap();
    let now = chrono::Utc::now();
    let clock = Arc::new(ManualCognitiveClock::new(now));
    rt.cognition = CognitiveRuntimeService::with_clock(
        rt.store.clone(),
        rt.cognition.resident_limit,
        rt.configuration.clone(),
        clock.clone(),
    )
    .unwrap();
    let sequence = rt.store.authority_seq(subject_a).await.unwrap();
    let mut intent = query(subject_a, a.clone());
    intent
        .expression
        .cues
        .push(Cue::Tag(TagCue { tag: tag.tag_id }));
    let result = rt.execute_query(intent, None).await.unwrap();
    assert_eq!(result.result.results.len(), 1);
    let row=sqlx::query("SELECT signals,returned_revision_refs,created_at,expires_at FROM query_feedback_records WHERE subject_id=$1 AND query_id=$2").bind(subject_a.0).bind(result.bound.query_id).fetch_one(rt.store.pool()).await.unwrap();
    let signals: serde_json::Value = row.try_get("signals").unwrap();
    assert!(!signals.to_string().contains("PRIVATE"));
    let created: chrono::DateTime<chrono::Utc> = row.try_get("created_at").unwrap();
    let expires: chrono::DateTime<chrono::Utc> = row.try_get("expires_at").unwrap();
    assert_eq!((expires - created).num_seconds(), 604800);
    assert_eq!(rt.store.authority_seq(subject_a).await.unwrap(), sequence);
    let event = feedback(subject_a, a.clone(), result.bound.query_id, now);
    assert_eq!(rt.cognition.use_feedback(event.clone()).await.unwrap().0, 1);
    assert_eq!(rt.cognition.use_feedback(event.clone()).await.unwrap().1, 1);
    let mut reopened = open_runtime(&url, &root).await;
    reopened.cognition = CognitiveRuntimeService::with_clock(
        reopened.store.clone(),
        reopened.cognition.resident_limit,
        reopened.configuration.clone(),
        clock.clone(),
    )
    .unwrap();
    assert_eq!(
        reopened
            .cognition
            .use_feedback(feedback(subject_a, a.clone(), result.bound.query_id, now))
            .await
            .unwrap()
            .0,
        1
    );
    let plan = rt
        .require_memory()
        .unwrap()
        .plan_concepts(subject_a, a.clone())
        .await
        .unwrap();
    assert_eq!(
        plan.model_input["queryFeedback"][0]["query_id"],
        serde_json::json!(result.bound.query_id)
    );
    assert_eq!(
        plan.model_input["queryFeedback"][0]["signals"]["explicit_tags"][0]["tag"],
        serde_json::json!(tag.tag_id)
    );
    assert!(
        plan.tags
            .iter()
            .any(|candidate| candidate.target.tag_id == tag.tag_id)
    );
    let presented = rt
        .execute_query(query(subject_a, a.clone()), None)
        .await
        .unwrap();
    let mut presentation = feedback(subject_a, a.clone(), presented.bound.query_id, now);
    presentation.events[0].use_kind = UseKind::Presented;
    rt.cognition.use_feedback(presentation).await.unwrap();
    let linked = nous_runtime::linked_query_feedback(&rt.store, subject_a, &a, now)
        .await
        .unwrap();
    assert!(
        linked
            .iter()
            .all(|feedback| feedback.query_id != presented.bound.query_id)
    );
    let mut conflict = event.clone();
    conflict.events[0].query_id = Some(uuid::Uuid::new_v4());
    assert!(matches!(
        rt.cognition.use_feedback(conflict).await,
        Err(Error::Domain(error)) if error.code == nous_core::DomainErrorCode::OperationIdConflict
    ));
    assert!(matches!(
        rt.cognition
            .use_feedback(feedback(subject_a, b.clone(), result.bound.query_id, now))
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        rt.cognition
            .use_feedback(feedback(subject_b, foreign, result.bound.query_id, now))
            .await,
        Err(Error::Invalid(_))
    ));
    // A validated pool is private; only the finalized response becomes feedback membership.
    let mut pool = query(subject_a, a.clone());
    pool.expression.targets.push(QueryTarget::Exact {
        reference: b.clone(),
    });
    pool.result_need.limit = 1;
    let execution = rt.execute_query(pool, Some(2)).await.unwrap();
    let query_id = execution.bound.query_id;
    assert_eq!(execution.result.results.len(), 2);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM query_feedback_records WHERE subject_id=$1 AND query_id=$2)",
    )
    .bind(subject_a.0)
    .bind(query_id)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert!(!exists);
    let (_, ticket) = rt
        .cognition
        .retain_query(execution, test_support::query_lease())
        .unwrap();
    let final_result = rt
        .cognition
        .finalize_query(
            subject_a,
            ticket.unwrap(),
            vec![(a.clone(), 1.0)],
            vec![],
            nous_runtime::CognitiveContributors {
                shared: None,
                memory: Some(rt.require_memory().unwrap()),
                material: Some(&rt.material),
            },
        )
        .await
        .unwrap();
    assert_eq!(final_result.results.len(), 1);
    assert_eq!(final_result.results[0].reference, a);
    assert!(matches!(
        rt.cognition
            .use_feedback(feedback(subject_a, b, query_id, now))
            .await,
        Err(Error::Invalid(_))
    ));
    clock
        .advance_by(subject_a, chrono::Duration::days(8))
        .unwrap();
    let later = rt.cognition.now(subject_a);
    assert!(matches!(
        rt.cognition
            .use_feedback(feedback(subject_a, a.clone(), result.bound.query_id, later))
            .await,
        Err(Error::Invalid(_))
    ));
    assert_eq!(rt.cognition.use_feedback(event).await.unwrap().1, 1);
    rt.execute_query(query(subject_a, a), None).await.unwrap();
    let expired: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM query_feedback_records WHERE subject_id=$1 AND query_id=$2)",
    )
    .bind(subject_a.0)
    .bind(result.bound.query_id)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert!(!expired);
    let accepted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cognitive_use_events WHERE subject_id=$1 AND query_id=$2",
    )
    .bind(subject_a.0)
    .bind(result.bound.query_id)
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(accepted, 2);
    assert_eq!(
        rt.configuration
            .snapshot_for_subject(subject_a)
            .unwrap()
            .get(QUERY_FEEDBACK_RETENTION)
            .unwrap(),
        604800
    );
}
