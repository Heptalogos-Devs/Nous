// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_core::TemporalExtent;
use nous_kernel::NousRuntime;
use nous_runtime::{
    CognitiveClock, MaintenanceDisposition, MaintenanceRequest, ManualCognitiveClock,
};
use sqlx::Row;
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{observation, runtime_with_clock};
use test_support::query::subject as create_subject;

#[tokio::test]
async fn experience_capture_cursor_idle_and_reopen_are_stable() {
    let (root, url, _postgres) = database().await;
    let start = Utc::now().trunc_subsecs(6);
    let clock = Arc::new(ManualCognitiveClock::new(start));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let first_session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let input = observation(subject, Some(first_session.session_id));
    let key = uuid::Uuid::new_v4();
    let accepted = rt
        .material
        .record_observation_once(input.clone(), Some(key))
        .await
        .unwrap();
    clock.advance_by(subject, Duration::seconds(30)).unwrap();
    let replay = rt
        .material
        .record_observation_once(input, Some(key))
        .await
        .unwrap();
    assert_eq!(
        accepted.occurrence.occurrence_id,
        replay.occurrence.occurrence_id
    );
    assert_eq!(replay.occurrence.observed_at, start);
    rt.material
        .record_observation(observation(subject, None))
        .await
        .unwrap();
    let second_session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    rt.material
        .record_observation(observation(subject, Some(second_session.session_id)))
        .await
        .unwrap();
    let rows = sqlx::query(
        "SELECT recorded_seq FROM experience_items WHERE subject_id=$1 ORDER BY recorded_seq",
    )
    .bind(subject.0)
    .fetch_all(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[1].get::<i64, _>("recorded_seq") > rows[0].get::<i64, _>("recorded_seq") + 1);
    let progress = rt
        .cognition
        .segment_experience(subject, "interaction", 128, false)
        .await
        .unwrap();
    assert_eq!(progress.processed_count, 2);
    assert!(progress.ready.is_empty());
    assert!(progress.next_due.is_some());
    let repeated = rt
        .cognition
        .segment_experience(subject, "interaction", 128, false)
        .await
        .unwrap();
    assert_eq!(repeated.processed_count, 0);
    clock.advance_by(subject, Duration::minutes(30)).unwrap();
    drop(rt);
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let progress = rt
        .cognition
        .segment_experience(subject, "interaction", 128, false)
        .await
        .unwrap();
    assert_eq!(progress.ready.len(), 1);
    assert_eq!(progress.ready[0].members.len(), 2);
    assert_eq!(
        progress.ready[0].boundary_reason.as_deref(),
        Some("hard_idle")
    );
    assert_eq!(progress.processed_count, 0);
    let organized = rt.organize_experience(subject, 128, false).await.unwrap();
    assert_eq!(organized.episodes.len(), 1);
    assert_eq!(organized.episodes[0].revision.formed_at, clock.now(subject));
    assert!(
        rt.organize_experience(subject, 128, false)
            .await
            .unwrap()
            .episodes
            .is_empty()
    );
    assert!(
        rt.cognition
            .maintenance_needs(subject)
            .await
            .unwrap()
            .iter()
            .any(|need| need.kind == "journal_review")
    );
    assert_owner_timestamp_replay(&rt, &clock, subject, accepted.occurrence.occurrence_id).await;
}

#[tokio::test]
async fn maintenance_claims_use_execution_leases_and_keep_new_triggers() {
    let (root, url, _postgres) = database().await;
    let start = Utc::now().trunc_subsecs(6);
    let clock = Arc::new(ManualCognitiveClock::new(start));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let request = MaintenanceRequest {
        subject,
        kind: "journal_review".into(),
        scope_kind: "track".into(),
        scope_ref: "interaction".into(),
        trigger_authority_seq: 1,
        due_at: start + Duration::hours(1),
        priority: 30,
    };
    let id = rt
        .cognition
        .enqueue_maintenance(request.clone())
        .await
        .unwrap();
    let kinds = vec!["journal_review".into()];
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_by(subject, Duration::hours(1)).unwrap();
    let (a, b) = tokio::join!(
        rt.cognition.lease_maintenance(subject, &kinds, 1, 60, None),
        rt.cognition.lease_maintenance(subject, &kinds, 1, 60, None)
    );
    let leased: Vec<_> = a.unwrap().into_iter().chain(b.unwrap()).collect();
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].need_id, id);
    clock.advance_by(subject, Duration::days(30)).unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    let newer = MaintenanceRequest {
        trigger_authority_seq: 2,
        ..request
    };
    assert_eq!(rt.cognition.enqueue_maintenance(newer).await.unwrap(), id);
    rt.cognition
        .acknowledge_maintenance(&leased[0], MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    rt.cognition
        .acknowledge_maintenance(&leased[0], MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    assert!(
        rt.cognition
            .acknowledge_maintenance(&leased[0], MaintenanceDisposition::Obsolete)
            .await
            .is_err()
    );
    let reclaimed = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap();
    assert_eq!(reclaimed.len(), 1);
    assert_eq!(reclaimed[0].trigger_authority_seq, 2);
    sqlx::query("UPDATE maintenance_needs SET lease_until=clock_timestamp()-interval '1 second' WHERE need_id=$1")
        .bind(id).execute(rt.store.pool()).await.unwrap();
    let retry = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap();
    assert_eq!(retry[0].attempt_count, 3);
    assert!(
        rt.cognition
            .acknowledge_maintenance(&reclaimed[0], MaintenanceDisposition::Satisfied)
            .await
            .is_err()
    );
    rt.cognition
        .acknowledge_maintenance(&retry[0], MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    assert!(
        rt.cognition
            .maintenance_needs(subject)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn ended_work_context_wakes_and_closes_experience_before_idle_deadline() {
    use nous_core::OperationId;
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let context = rt
        .cognition
        .create_work_context(nous_runtime::CreateWorkContextInput {
            operation_id: OperationId::new(),
            subject,
            purpose: "Bounded work".into(),
            unresolved_questions: vec![],
            constraints: serde_json::json!({}),
            resume_conditions: vec![],
            budget_summary: serde_json::json!({}),
            context_text: String::new(),
            entity_anchors: vec![],
            tag_anchors: vec![],
            cognition_anchors: vec![],
        })
        .await
        .unwrap();
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    rt.cognition
        .set_active_work_context(
            subject,
            session.session_id,
            Some(context.work_context_id),
            session.runtime_revision,
            OperationId::new(),
        )
        .await
        .unwrap();
    rt.material
        .record_observation_once(
            observation(subject, Some(session.session_id)),
            Some(uuid::Uuid::new_v4()),
        )
        .await
        .unwrap();
    let initial = rt.organize_experience(subject, 128, false).await.unwrap();
    assert!(initial.episodes.is_empty());
    let claimed = rt
        .cognition
        .lease_maintenance(subject, &["episode_segment".into()], 1, 60, None)
        .await
        .unwrap();
    rt.cognition
        .acknowledge_maintenance(
            &claimed[0],
            MaintenanceDisposition::Pending {
                due_at: initial.next_due.unwrap(),
                problem_code: None,
            },
        )
        .await
        .unwrap();
    rt.cognition
        .end_work_context(
            subject,
            context.work_context_id,
            context.revision,
            OperationId::new(),
        )
        .await
        .unwrap();
    let needs = rt
        .cognition
        .lease_maintenance(subject, &["episode_segment".into()], 1, 60, None)
        .await
        .unwrap();
    assert_eq!(needs.len(), 1);
    let closed = rt.organize_experience(subject, 128, false).await.unwrap();
    assert_eq!(closed.episodes.len(), 1);
    assert_eq!(
        closed.episodes[0].revision.boundary_explanation,
        "work_context_ended"
    );
}

async fn assert_owner_timestamp_replay(
    rt: &NousRuntime,
    clock: &ManualCognitiveClock,
    subject: nous_core::SubjectId,
    occurrence: nous_core::OccurrenceId,
) {
    let input = test_support::form_input(
        subject,
        occurrence,
        nous_core::OperationId::new(),
        "owner-timed grounded memory",
    );
    let at = clock.now(subject);
    let memory = rt
        .require_memory()
        .unwrap()
        .form_memory(input.clone())
        .await
        .unwrap();
    assert_eq!(memory.revision.formed_at, at);
    assert_eq!(memory.revision.recorded_at, at);
    clock.advance_by(subject, Duration::days(1)).unwrap();
    rt.require_memory()
        .unwrap()
        .revise_memory(nous_memory::ReviseMemoryInput {
            producer: None,
            operation_id: nous_core::OperationId::new(),
            subject,
            memory_id: memory.object.memory_id,
            expected_object_epoch: memory.object.object_epoch,
            intent: nous_memory::RevisionIntent::Correct,
            formation_mode: input.formation_mode,
            grounding_occurrence_id: input.grounding_occurrence_id,
            semantic_role: input.semantic_role.clone(),
            representation_text: "corrected owner-timed memory".into(),
            title: None,
            basis: input.basis.clone(),
            aboutness: vec![],
            valid_time: TemporalExtent::Unknown,
            epistemic_class: input.epistemic_class,
        })
        .await
        .unwrap();
    let replay = rt
        .require_memory()
        .unwrap()
        .form_memory(input)
        .await
        .unwrap();
    assert_eq!(
        replay.revision.memory_revision_id,
        memory.revision.memory_revision_id
    );
    assert_eq!(replay.revision.formed_at, at);
    assert_eq!(replay.revision.recorded_at, at);
    assert_schema_clock(rt, clock, subject, occurrence).await;
}

async fn assert_schema_clock(
    rt: &NousRuntime,
    clock: &ManualCognitiveClock,
    subject: nous_core::SubjectId,
    occurrence: nous_core::OccurrenceId,
) {
    let schema_input = nous_memory::CreateSchemaInput {
        operation_id: nous_core::OperationId::new(),
        subject,
        content: nous_memory::SchemaContent {
            producer: None,
            title: None,
            structural_claim: "Owner-assigned cognitive formation time".into(),
            applicability_scope: nous_memory::SchemaScope {
                description: "Timestamp owner contract".into(),
                aboutness: vec![],
                tags: vec![],
                valid_time: TemporalExtent::Unknown,
            },
            boundary_definition: "Evidence-backed imported schema".into(),
            formation_kind: nous_memory::SchemaFormationKind::ExplicitImport,
            evidence_links: vec![nous_memory::SchemaEvidenceLinkInput {
                role: nous_memory::SchemaEvidenceRole::Support,
                basis: nous_core::RevisionBasis::Evidence(nous_core::EvidenceRef {
                    epistemic_relation: None,
                    occurrence_id: occurrence,
                    locator: nous_core::EvidenceLocator::WholeOccurrence,
                    basis_role: nous_core::BasisRole::Direct,
                }),
            }],
        },
    };
    let schema = rt
        .require_memory()
        .unwrap()
        .create_schema(schema_input.clone())
        .await
        .unwrap();
    assert_eq!(schema.revision.formed_at, clock.now(subject));
    assert_eq!(schema.revision.recorded_at, clock.now(subject));
    let at = clock.now(subject);
    clock.advance_by(subject, Duration::days(1)).unwrap();
    let replay_schema = rt
        .require_memory()
        .unwrap()
        .create_schema(schema_input.clone())
        .await
        .unwrap();
    assert_eq!(replay_schema.revision.formed_at, at);
    let revised_schema = rt
        .require_memory()
        .unwrap()
        .revise_schema(nous_memory::ReviseSchemaInput {
            operation_id: nous_core::OperationId::new(),
            subject,
            schema_id: schema.schema.schema_id,
            expected_object_epoch: schema.schema.object_epoch,
            intent: nous_memory::RevisionIntent::Correct,
            copy_link_ids: schema
                .evidence_links
                .iter()
                .map(|link| link.link_id)
                .collect(),
            content: nous_memory::SchemaContent {
                formation_kind: schema.revision.formation_kind,
                producer: None,
                evidence_links: Vec::new(),
                title: Some("Revised at cognition time".into()),
                structural_claim: schema_input.content.structural_claim,
                applicability_scope: schema_input.content.applicability_scope,
                boundary_definition: schema_input.content.boundary_definition,
            },
        })
        .await
        .unwrap();
    assert_eq!(revised_schema.revision.formed_at, clock.now(subject));
    assert_eq!(revised_schema.revision.recorded_at, clock.now(subject));
}
