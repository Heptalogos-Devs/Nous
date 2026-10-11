// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_core::TemporalExtent;
use nous_memory::EpisodePartitionSource;
use nous_runtime::{
    CognitiveClock, MaintenanceDisposition, MaintenanceRequest, ManualCognitiveClock,
};
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{observation, runtime_with_clock};
use test_support::query::subject as create_subject;

#[tokio::test]
async fn drafts_commit_replay_and_ack_reclaim_runtime_rows() {
    use nous_core::{CognitiveRef, OperationId};
    use nous_memory::{EpisodeInput, EpisodeMemberInput};
    let (root, url, _postgres) = database().await;
    let at = Utc::now().trunc_subsecs(6);
    let clock = Arc::new(ManualCognitiveClock::new(at));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let session = rt
        .cognition
        .open_session(subject, serde_json::json!({}))
        .await
        .unwrap();
    let mut occurrences = Vec::new();
    for offset in [-20, 0, -10] {
        let mut input = observation(subject, Some(session.session_id));
        input.occurrence.observed_at = Some(at + Duration::minutes(offset));
        occurrences.push(
            rt.material
                .record_observation(input)
                .await
                .unwrap()
                .occurrence
                .occurrence_id,
        );
    }
    let progress = rt
        .cognition
        .segment_experience(subject, "interaction", 256, true)
        .await
        .unwrap();
    let draft = &progress.ready[0];
    assert_eq!(
        draft.members,
        vec![occurrences[0], occurrences[2], occurrences[1]]
    );
    let input = EpisodeInput {
        operation_id: OperationId(uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_OID,
            format!("episode-draft:{}", draft.draft_id).as_bytes(),
        )),
        subject,
        track_key: draft.track_key.clone(),
        title: None,
        parent_episode_revision_id: None,
        experience_time: TemporalExtent::Interval {
            start: Some(draft.observed_start),
            end: Some(draft.observed_end),
        },
        boundary_explanation: draft.boundary_reason.clone().unwrap(),
        producer_signature_id: None,
        members: draft
            .members
            .iter()
            .map(|id| EpisodeMemberInput {
                reference: CognitiveRef::Occurrence(*id),
                role: "experience".into(),
            })
            .collect(),
        basis: draft
            .members
            .iter()
            .map(|id| {
                nous_core::RevisionBasis::Evidence(nous_core::EvidenceRef {
                    epistemic_relation: None,
                    occurrence_id: *id,
                    locator: nous_core::EvidenceLocator::WholeOccurrence,
                    basis_role: nous_core::BasisRole::Direct,
                })
            })
            .collect(),
    };
    let committed = rt
        .require_memory()
        .unwrap()
        .create_episode(input)
        .await
        .unwrap();
    // Reopen after the owner commit, before Runtime acknowledgement.
    drop(rt);
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    clock.advance_by(subject, Duration::days(1)).unwrap();
    let resumed = rt.organize_experience(subject, 256, true).await.unwrap();
    assert_eq!(
        resumed.episodes[0].revision.episode_revision_id,
        committed.revision.episode_revision_id
    );
    assert_eq!(resumed.episodes[0].revision.formed_at, at);
    rt.cognition
        .acknowledge_episode_draft(
            subject,
            draft.draft_id,
            committed.revision.episode_revision_id,
        )
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM episode_drafts)+(SELECT count(*) FROM episode_draft_members)",
    )
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(count, 0);
    let dangling: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM segmentation_cursors WHERE open_draft_id IS NOT NULL)",
    )
    .fetch_one(rt.store.pool())
    .await
    .unwrap();
    assert!(!dangling);
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one regression follows runtime state across acknowledgement, replay and expiration"
)]
async fn maintenance_terminal_retention_workflow_cleanup_and_execution_backoff() {
    let (root, url, _postgres) = database().await;
    let at = Utc::now().trunc_subsecs(6);
    let clock = Arc::new(ManualCognitiveClock::new(at));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let request = MaintenanceRequest {
        subject,
        kind: "journal_review".into(),
        scope_kind: "track".into(),
        scope_ref: "interaction".into(),
        trigger_authority_seq: 0,
        due_at: at,
        priority: 30,
    };
    let need_id = rt
        .cognition
        .enqueue_maintenance(request.clone())
        .await
        .unwrap();
    let kinds = vec![request.kind.clone()];
    let claimed = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    let workflow = rt
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            "maintenance-test",
            "input",
            &nous_core::WorkflowSnapshot {
                maintenance_claim: Some(nous_core::MaintenanceClaim {
                    need_id,
                    lease_token: claimed.lease_token.unwrap(),
                    trigger_authority_seq: claimed.trigger_authority_seq,
                    trigger_revision: claimed.trigger_revision,
                }),
                ..Default::default()
            },
            360,
        )
        .await
        .unwrap();
    let explicit = rt
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            "explicit-test",
            "input",
            &nous_core::WorkflowSnapshot::default(),
            360,
        )
        .await
        .unwrap();
    for (key, token) in [
        ("maintenance-test", workflow.lease.as_ref().unwrap().token),
        ("explicit-test", explicit.lease.as_ref().unwrap().token),
    ] {
        rt.store
            .save_model_workflow(
                &test_support::workflow_lease(subject, "memory", key, token),
                None,
                Some(&test_support::workflow_payload(
                    serde_json::json!({"status":"no_change"}),
                )),
                None,
                &[],
            )
            .await
            .unwrap();
    }
    rt.cognition
        .acknowledge_maintenance(&claimed, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    clock.advance_by(subject, Duration::days(100)).unwrap();
    rt.cognition
        .acknowledge_maintenance(&claimed, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT operation_key FROM model_workflow_operations WHERE subject_id=$1",
    )
    .bind(subject.0)
    .fetch_all(rt.store.pool())
    .await
    .unwrap();
    assert_eq!(keys, vec!["explicit-test"]);
    // Execution time, not accelerated cognition time, defines the replay window.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM maintenance_needs WHERE need_id=$1")
        .bind(need_id)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    sqlx::query("UPDATE maintenance_needs SET terminal_at=clock_timestamp()-interval '2 days' WHERE need_id=$1").bind(need_id).execute(rt.store.pool()).await.unwrap();
    let pending = rt
        .cognition
        .enqueue_maintenance(MaintenanceRequest {
            scope_ref: "pending".into(),
            ..request
        })
        .await
        .unwrap();
    let claimed = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(claimed.need_id, pending);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM maintenance_needs WHERE need_id=$1")
        .bind(need_id)
        .fetch_one(rt.store.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    rt.cognition
        .acknowledge_maintenance(
            &claimed,
            MaintenanceDisposition::Retry {
                delay_seconds: 30,
                problem_code: "provider_unavailable".into(),
            },
        )
        .await
        .unwrap();
    clock.advance_by(subject, Duration::days(100)).unwrap();
    rt.cognition
        .acknowledge_maintenance(
            &claimed,
            MaintenanceDisposition::Retry {
                delay_seconds: 30,
                problem_code: "provider_unavailable".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE maintenance_needs SET retry_not_before=clock_timestamp()-interval '1 second' WHERE need_id=$1").bind(pending).execute(rt.store.pool()).await.unwrap();
    let retry = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(retry.retry_count, 1);
    assert_eq!(retry.attempt_count, 2);
    let abandoned = rt
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            "superseded-test",
            "input",
            &nous_core::WorkflowSnapshot {
                maintenance_claim: Some(nous_core::MaintenanceClaim {
                    need_id: pending,
                    lease_token: retry.lease_token.unwrap(),
                    trigger_authority_seq: retry.trigger_authority_seq,
                    trigger_revision: retry.trigger_revision,
                }),
                ..Default::default()
            },
            360,
        )
        .await
        .unwrap();
    rt.store
        .release_model_workflow(&test_support::workflow_lease(
            subject,
            "memory",
            "superseded-test",
            abandoned.lease.as_ref().unwrap().token,
        ))
        .await
        .unwrap();
    rt.cognition
        .enqueue_maintenance(MaintenanceRequest {
            subject,
            kind: "journal_review".into(),
            scope_kind: "track".into(),
            scope_ref: "pending".into(),
            trigger_authority_seq: 1,
            due_at: clock.now(subject),
            priority: 30,
        })
        .await
        .unwrap();
    rt.cognition
        .acknowledge_maintenance(&retry, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let survives: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM model_workflow_operations WHERE operation_key='superseded-test')").fetch_one(rt.store.pool()).await.unwrap();
    assert!(!survives);
    assert_eq!(
        rt.cognition.maintenance_needs(subject).await.unwrap()[0].state,
        "pending"
    );
    let original_digest = "a".repeat(64);
    let replacement_digest = "b".repeat(64);
    let blocked = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, Some(&original_digest))
        .await
        .unwrap()
        .remove(0);
    rt.cognition
        .acknowledge_maintenance(
            &blocked,
            MaintenanceDisposition::Blocked {
                problem_code: "maintenance_retry_exhausted".into(),
            },
        )
        .await
        .unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, Some(&original_digest))
            .await
            .unwrap()
            .is_empty()
    );
    let reactivated = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, Some(&replacement_digest))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        reactivated.trigger_authority_seq,
        blocked.trigger_authority_seq
    );
    assert!(reactivated.trigger_revision > blocked.trigger_revision);
    assert_eq!(reactivated.retry_count, 0);
}

#[path = "maintenance_workflows/scopes.rs"]
mod scopes;
