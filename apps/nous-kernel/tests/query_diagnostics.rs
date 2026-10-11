// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, Utc};
use nous_core::{Cue, EntityCue, QueryConstraints, TimePredicate};
use test_support::query::{subject, text_query};
use test_support::{database, form_input, observation, open_runtime, open_runtime_with_serving};

#[tokio::test]
async fn query_distinguishes_provider_unavailable_from_unknown() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "diagnostic phrase").await;
    runtime
        .require_memory()
        .expect("Memory")
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            nous_core::OperationId::new(),
            "diagnostic phrase",
        ))
        .await
        .expect("memory");
    let result = runtime.query(text_query(subject)).await.expect("query");
    assert_eq!(result.status, nous_core::QueryStatus::Degraded);
    assert!(
        result
            .degradation
            .iter()
            .any(|value| value.code == "dense_lane_unavailable")
    );
    let mut associative = text_query(subject);
    associative.exploration = nous_core::ExplorationIntent::BoundedAssociative;
    let unavailable = runtime.query(associative).await.expect("associative query");
    assert!(
        unavailable
            .degradation
            .iter()
            .any(|value| value.code == "topologywave_lane_unavailable")
    );
}

#[tokio::test]
async fn query_reports_validation_budget_exhaustion() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let mut tx = runtime.store.begin().await.expect("fixture transaction");
    sqlx::query("INSERT INTO memory_objects(memory_id,subject_id,cognitive_role,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,accessibility_mode,created_at) SELECT md5('budget-memory-'||n)::uuid,$1,'declarative',md5('budget-rev-'||n)::uuid,1,'accepted','valid','suppressed','normal','auto',now() FROM generate_series(1,40) AS values(n)")
        .bind(subject.0)
        .execute(&mut *tx)
        .await
        .expect("budget memories");
    sqlx::query("INSERT INTO memory_revisions(memory_revision_id,memory_id,subject_id,revision_no,parent_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end,formed_at,recorded_at) SELECT md5('budget-rev-'||n)::uuid,md5('budget-memory-'||n)::uuid,$1,1,NULL,NULL,'synthesized',NULL,'budget',NULL,'budget memory','inferred','unknown',NULL,NULL,now(),now() FROM generate_series(1,40) AS values(n)")
        .bind(subject.0)
        .execute(&mut *tx)
        .await
        .expect("budget revisions");
    sqlx::query("INSERT INTO memory_revision_aboutness(memory_revision_id,entity_ref) SELECT md5('budget-rev-'||n)::uuid,'entity:budget' FROM generate_series(1,40) AS values(n)")
        .execute(&mut *tx)
        .await
        .expect("budget aboutness");
    sqlx::query("INSERT INTO memory_objects(memory_id,subject_id,cognitive_role,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,accessibility_mode,created_at) SELECT md5('budget-temporal-memory-'||n)::uuid,$1,'declarative',md5('budget-temporal-rev-'||n)::uuid,1,'accepted','valid','normal','normal','auto',now() FROM generate_series(1,40) AS values(n)")
        .bind(subject.0)
        .execute(&mut *tx)
        .await
        .expect("temporal budget memories");
    sqlx::query("INSERT INTO memory_revisions(memory_revision_id,memory_id,subject_id,revision_no,parent_revision_id,revision_intent,formation_mode,grounding_occurrence_id,semantic_role,title,representation_text,epistemic_class,valid_time_kind,valid_time_start,valid_time_end,formed_at,recorded_at) SELECT md5('budget-temporal-rev-'||n)::uuid,md5('budget-temporal-memory-'||n)::uuid,$1,1,NULL,NULL,'synthesized',NULL,'budget',NULL,'temporal budget memory','inferred','instant',now(),NULL,now(),now() FROM generate_series(1,40) AS values(n)")
        .bind(subject.0)
        .execute(&mut *tx)
        .await
        .expect("temporal budget revisions");
    tx.commit().await.expect("budget commit");
    let mut query = text_query(subject);
    query.expression.cues = vec![
        Cue::Text(nous_core::TextCue {
            text: "Recall cognition for this entity".into(),
        }),
        Cue::Entity(EntityCue {
            entity_ref: nous_core::EntityRef::new("entity:budget").unwrap(),
        }),
    ];
    query.result_need.limit = 8;
    query.expression.constraints = QueryConstraints {
        valid: Some(TimePredicate::Range {
            start: Some(Utc::now() - Duration::seconds(5)),
            end: Some(Utc::now() + Duration::seconds(5)),
        }),
        ..Default::default()
    };
    let result = runtime.query(query).await.expect("budget query");
    assert_eq!(
        result.status,
        nous_core::QueryStatus::Partial,
        "result={result:?}"
    );
    assert!(
        result
            .degradation
            .iter()
            .any(|value| value.code == "validation_budget_exhausted")
    );
}
