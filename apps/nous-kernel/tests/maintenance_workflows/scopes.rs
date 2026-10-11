// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one regression covers blocked source scopes and their event-driven recovery"
)]
async fn oversized_repair_and_journal_scopes_block_until_real_triggers() {
    use nous_core::{BasisRole, CognitionDependency, CognitiveRef, OperationId, RevisionBasis};
    use nous_memory::{JournalInput, JournalPoint, JournalPointRole};
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
    let mut episodes = Vec::new();
    for offset in [-20, 0] {
        let mut input = observation(subject, Some(session.session_id));
        input.occurrence.observed_at = Some(at + Duration::minutes(offset));
        rt.material.record_observation(input).await.unwrap();
        episodes.extend(
            rt.organize_experience(subject, 256, true)
                .await
                .unwrap()
                .episodes,
        );
    }
    let memory = rt.require_memory().unwrap();
    let journal = memory
        .commit_journal(JournalInput {
            operation_id: OperationId::new(),
            subject,
            expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
            target: None,
            sources: episodes
                .iter()
                .map(|episode| EpisodePartitionSource {
                    revision: episode.revision.episode_revision_id,
                    expected_epoch: episode.object.object_epoch,
                })
                .collect(),
            title: None,
            narrative: "Complete source scope.".into(),
            producer: None,
            points: vec![JournalPoint {
                role: JournalPointRole::Summary,
                text: "Both experiences.".into(),
                basis: episodes
                    .iter()
                    .map(|episode| {
                        RevisionBasis::CognitionDependency(CognitionDependency {
                            epistemic_relation: None,
                            target_revision: CognitiveRef::EpisodeRevision(
                                episode.revision.episode_revision_id,
                            ),
                            basis_role: BasisRole::Direct,
                        })
                    })
                    .collect(),
            }],
        })
        .await
        .unwrap();
    let episode = &episodes[1];
    memory
        .revise_episode(nous_memory::ReviseEpisodeInput {
            operation_id: OperationId::new(),
            subject,
            episode_id: episode.object.episode_id,
            expected_object_epoch: episode.object.object_epoch,
            intent: "reinterpret".into(),
            title: Some("Updated source".into()),
            parent_episode_revision_id: None,
            experience_time: episode.revision.experience_time.clone(),
            boundary_explanation: episode.revision.boundary_explanation.clone(),
            producer_signature_id: None,
            members: episode
                .members
                .iter()
                .map(|member| nous_memory::EpisodeMemberInput {
                    reference: member.reference.clone(),
                    role: member.role.clone(),
                })
                .collect(),
            basis: episode.basis.clone(),
        })
        .await
        .unwrap();
    clock.advance_by(subject, Duration::seconds(300)).unwrap();
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            "journal.max_episode_count",
            serde_json::json!(1),
            None,
        )
        .await
        .unwrap();
    let scope = journal.object.journal_id.0.to_string();
    let plan = memory
        .plan_journal_review(subject, "journal_revalidate", &scope)
        .await
        .unwrap();
    assert_eq!(plan.status, "blocked");
    assert_eq!(
        plan.problem.as_deref(),
        Some("journal_scope_bound_exceeded")
    );
    assert!(plan.next_due.is_none());
    let kinds = vec!["journal_revalidate".into()];
    let claim = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    rt.cognition
        .acknowledge_maintenance(
            &claim,
            MaintenanceDisposition::Blocked {
                problem_code: plan.problem.unwrap(),
            },
        )
        .await
        .unwrap();
    clock.advance_by(subject, Duration::days(30)).unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        memory
            .journal(subject, journal.object.journal_id, None)
            .await
            .unwrap()
            .object
            .integrity_state,
        nous_core::IntegrityState::RevalidationRequired
    );
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            "journal.max_episode_count",
            serde_json::json!(2),
            None,
        )
        .await
        .unwrap();
    let claim = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        memory
            .plan_journal_review(subject, "journal_revalidate", &scope)
            .await
            .unwrap()
            .episodes
            .len(),
        2
    );
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            "journal.max_span_seconds",
            serde_json::json!(60),
            None,
        )
        .await
        .unwrap();
    let plan = memory
        .plan_journal_review(subject, "journal_revalidate", &scope)
        .await
        .unwrap();
    assert_eq!(plan.status, "blocked");
    assert_eq!(plan.problem.as_deref(), Some("journal_scope_span_exceeded"));
    rt.cognition
        .acknowledge_maintenance(
            &claim,
            MaintenanceDisposition::Blocked {
                problem_code: plan.problem.unwrap(),
            },
        )
        .await
        .unwrap();
    clock.advance_by(subject, Duration::days(30)).unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    // A relevant source lifecycle event wakes the blocked revalidation obligation.
    memory
        .suppress_episode(
            subject,
            episodes[0].object.episode_id,
            OperationId::new(),
            episodes[0].object.object_epoch,
        )
        .await
        .unwrap();
    let claim = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    let ready = memory
        .plan_journal_review(subject, "journal_revalidate", &scope)
        .await
        .unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.episodes.len(), 1);
    rt.cognition
        .acknowledge_maintenance(&claim, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    // Historical repair scope bounds apply to experience extent, not age.
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            "episode.max_neighbor_span_seconds",
            serde_json::json!(60),
            None,
        )
        .await
        .unwrap();
    let mut input = observation(subject, Some(session.session_id));
    input.occurrence.observed_at = Some(at - Duration::minutes(10));
    rt.material.record_observation(input).await.unwrap();
    rt.organize_experience(subject, 256, false).await.unwrap();
    let kinds = vec!["episode_resegment".into()];
    let claims = rt
        .cognition
        .lease_maintenance(subject, &kinds, 8, 60, None)
        .await
        .unwrap();
    let claim = claims
        .iter()
        .find(|need| need.scope_kind == "track")
        .unwrap();
    let plan = memory
        .plan_episode_review(subject, "track", "interaction", claim.trigger_authority_seq)
        .await
        .unwrap();
    assert_eq!(plan.status, "blocked");
    assert_eq!(
        plan.problem.as_deref(),
        Some("historical_repair_scope_exceeded")
    );
    rt.cognition
        .acknowledge_maintenance(
            claim,
            MaintenanceDisposition::Blocked {
                problem_code: plan.problem.unwrap(),
            },
        )
        .await
        .unwrap();
    for other in claims.iter().filter(|need| need.need_id != claim.need_id) {
        rt.cognition
            .acknowledge_maintenance(other, MaintenanceDisposition::Satisfied)
            .await
            .unwrap();
    }
    clock.advance_by(subject, Duration::days(30)).unwrap();
    assert!(
        rt.cognition
            .lease_maintenance(subject, &kinds, 1, 60, None)
            .await
            .unwrap()
            .is_empty()
    );
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            "episode.max_neighbor_span_seconds",
            serde_json::json!(86400),
            None,
        )
        .await
        .unwrap();
    let resumed = rt
        .cognition
        .lease_maintenance(subject, &kinds, 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        memory
            .plan_episode_review(
                subject,
                "track",
                "interaction",
                resumed.trigger_authority_seq
            )
            .await
            .unwrap()
            .status,
        "ready"
    );
}
