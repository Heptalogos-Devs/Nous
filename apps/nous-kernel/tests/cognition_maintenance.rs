// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_core::TemporalExtent;
use nous_kernel::NousRuntime;
use nous_memory::{EpisodePartitionSource, EpisodeView};
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::{
    consolidation_memory, consolidation_producer, consolidation_schema, journal_source_episode,
    observation, runtime_with_clock, runtime_with_clock_serving,
};
use test_support::query::subject as create_subject;

#[tokio::test]
async fn typed_cognition_actions_preserve_prior_commits_and_exact_schema_replay() {
    use nous_core::OperationId;
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock_serving(&url, &root, clock.clone(), true).await;
    let subject = create_subject(&rt).await;
    let episode = journal_source_episode(&rt, &clock, subject).await;
    let owner = rt.require_memory().unwrap();
    let first_input = consolidation_memory(&episode);
    let first = owner.form_memory(first_input.clone()).await.unwrap();
    let second = owner
        .form_memory(consolidation_memory(&episode))
        .await
        .unwrap();
    let schema_input = consolidation_schema(&episode);
    let schema = owner.create_schema(schema_input.clone()).await.unwrap();
    let mut invalid = consolidation_schema(&episode);
    invalid.content.boundary_definition.clear();
    assert!(owner.create_schema(invalid).await.is_err());
    assert_eq!(
        owner
            .form_memory(first_input.clone())
            .await
            .unwrap()
            .revision
            .memory_revision_id,
        first.revision.memory_revision_id
    );
    owner
        .link_revisions(
            subject,
            OperationId::new(),
            first.revision.memory_revision_id,
            second.revision.memory_revision_id,
            nous_memory::MemoryRelation::Elaborates,
        )
        .await
        .unwrap();
    let results = [
        Some(nous_core::CognitiveRef::MemoryRevision(
            first.revision.memory_revision_id,
        )),
        Some(nous_core::CognitiveRef::MemoryRevision(
            second.revision.memory_revision_id,
        )),
        Some(nous_core::CognitiveRef::CognitiveSchemaRevision(
            schema.revision.schema_revision_id,
        )),
    ];
    clock.advance_by(subject, Duration::days(1)).unwrap();
    assert_consolidation_context(&rt, subject, &results).await;
    let revised_input = nous_memory::ReviseSchemaInput {
        operation_id: OperationId::new(),
        subject,
        schema_id: schema.schema.schema_id,
        expected_object_epoch: schema.schema.object_epoch,
        intent: nous_memory::RevisionIntent::Rephrase,
        copy_link_ids: Vec::new(),
        content: nous_memory::SchemaContent {
            formation_kind: schema.revision.formation_kind,
            producer: Some(consolidation_producer()),
            evidence_links: schema_input.content.evidence_links.clone(),
            title: schema.revision.title.clone(),
            structural_claim: "A clarified recurring pattern.".into(),
            applicability_scope: schema.revision.applicability_scope.clone(),
            boundary_definition: schema.revision.boundary_definition.clone(),
        },
    };
    let revised = owner.revise_schema(revised_input.clone()).await.unwrap();
    assert!(revised.revision.producer_signature_id.is_some());
    assert_ne!(
        revised.revision.schema_revision_id,
        schema.revision.schema_revision_id
    );
    assert_eq!(
        owner
            .create_schema(schema_input)
            .await
            .unwrap()
            .revision
            .schema_revision_id,
        schema.revision.schema_revision_id
    );
    let mut next = revised_input.clone();
    next.operation_id = OperationId::new();
    next.expected_object_epoch = revised.schema.object_epoch;
    owner.revise_schema(next).await.unwrap();
    assert_eq!(
        owner
            .revise_schema(revised_input)
            .await
            .unwrap()
            .revision
            .schema_revision_id,
        revised.revision.schema_revision_id
    );
}

async fn assert_consolidation_context(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    results: &[Option<nous_core::CognitiveRef>],
) {
    use nous_protocol::{
        kernel as k, kernel::kernel_maintenance_service_server::KernelMaintenanceService,
    };
    rt.cognition
        .use_feedback(nous_runtime::UseFeedback {
            subject,
            session_id: None,
            consumer_ref: "consumer:test:context".into(),
            events: [
                nous_runtime::UseKind::Presented,
                nous_runtime::UseKind::Referenced,
            ]
            .into_iter()
            .map(|use_kind| nous_runtime::UseFeedbackEvent {
                query_id: None,
                event_id: nous_core::UseEventId::new(),
                reference: results[0].clone().unwrap(),
                use_kind,
                occurred_at: rt.cognition.now(subject),
                context: serde_json::json!({}),
            })
            .collect(),
        })
        .await
        .unwrap();
    let service = nous_kernel::transport::KernelService(rt.clone());
    let claim = service
        .claim_maintenance(tonic::Request::new(k::ClaimMaintenanceRequest {
            subject_id: subject.0.to_string(),
            allowed_kinds: vec!["memory_consolidate".into()],
            limit: 1,
            lease_seconds: 60,
            model_execution_digest: None,
        }))
        .await
        .unwrap()
        .into_inner();
    let need = claim.needs[0].clone();
    let plan = service
        .plan_maintenance(tonic::Request::new(k::PlanMaintenanceRequest {
            claimed: Some(need.clone()),
        }))
        .await
        .unwrap()
        .into_inner();
    let memory = plan
        .candidates
        .iter()
        .find(|candidate| candidate.key == results[0].as_ref().unwrap().to_string())
        .unwrap();
    assert_eq!(memory.r#use.len(), 1);
    assert_eq!(memory.r#use[0].kind, "referenced");
    assert_eq!(memory.r#use[0].count, 1);
    let schema = plan
        .candidates
        .iter()
        .find(|candidate| candidate.key == results[2].as_ref().unwrap().to_string())
        .unwrap();
    assert!(schema.text.contains("Applicability: The observed contexts"));
    assert!(
        schema
            .text
            .contains("Boundary: Applies to these observed contexts.")
    );
    assert!(schema.text.contains("Tags:"));
    assert!(schema.text.len() <= 4096);
    assert_eq!(schema.formation_mode, "synthesized");
    assert!(plan.candidates.iter().all(|candidate| {
        matches!(
            candidate
                .target
                .as_ref()
                .unwrap()
                .reference
                .as_ref()
                .unwrap()
                .kind
                .as_str(),
            "memory_revision" | "cognitive_schema_revision"
        )
    }));
    for candidate in &plan.candidates {
        let target = candidate.target.as_ref().unwrap();
        assert!(!target.object_id.is_empty());
        let own = target.reference.as_ref().unwrap();
        for key in &candidate.eligible_basis_keys {
            let basis = plan
                .basis
                .iter()
                .find(|basis| &basis.key == key)
                .unwrap()
                .basis
                .as_ref()
                .unwrap();
            if let Some(nous_protocol::public::revision_basis::Basis::CognitionDependency(
                dependency,
            )) = &basis.basis
            {
                assert_ne!(dependency.target_revision.as_ref().unwrap(), own);
            }
        }
    }
    assert_consolidation_policy(rt, subject, &service, &need).await;
    service
        .finish_maintenance(tonic::Request::new(k::FinishMaintenanceRequest {
            claimed: Some(need),
            disposition: "satisfied".into(),
            retry_delay_seconds: 0,
            ..Default::default()
        }))
        .await
        .unwrap();
}

async fn assert_consolidation_policy(
    rt: &NousRuntime,
    subject: nous_core::SubjectId,
    service: &nous_kernel::transport::KernelService,
    need: &nous_protocol::kernel::MaintenanceNeed,
) {
    use nous_protocol::{
        kernel as k, kernel::kernel_maintenance_service_server::KernelMaintenanceService,
    };
    let mut policy = rt
        .configuration
        .snapshot_for_subject(subject)
        .unwrap()
        .get(nous_memory::CONSOLIDATION_CONTEXT)
        .unwrap();
    policy.candidate_text_chars = 64;
    policy.basis_limit = 1;
    policy.provenance_root_limit = 1;
    rt.configuration
        .set_subject_override(
            nous_core::OperationId::new(),
            subject,
            nous_memory::CONSOLIDATION_CONTEXT.path(),
            serde_json::to_value(policy).unwrap(),
            None,
        )
        .await
        .unwrap();
    let bounded = service
        .plan_maintenance(tonic::Request::new(k::PlanMaintenanceRequest {
            claimed: Some(need.clone()),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!bounded.candidates.is_empty());
    assert!(
        bounded
            .candidates
            .iter()
            .all(|c| c.text.chars().count() <= 64)
    );
    assert!(
        bounded
            .candidates
            .iter()
            .any(|candidate| candidate.text_truncated),
        "bounded candidate prefixes must remain distinguishable from complete cognition"
    );
    assert_eq!(bounded.basis.len(), 1);
    assert!(bounded.basis_catalog_partial);
    assert!(bounded.provenance_roots.len() <= 1);
}

async fn manual_episode_sources(
    rt: &NousRuntime,
    clock: &ManualCognitiveClock,
    subject: nous_core::SubjectId,
) -> (nous_core::OccurrenceId, EpisodeView, EpisodeView) {
    use nous_core::{BasisRole, CognitionDependency, CognitiveRef, OperationId, RevisionBasis};
    use nous_memory::{EpisodeInput, EpisodeMemberInput};
    let occurrence = rt
        .material
        .record_observation_once(observation(subject, None), Some(uuid::Uuid::new_v4()))
        .await
        .unwrap()
        .occurrence
        .occurrence_id;
    let memory = rt.require_memory().unwrap();
    let evidence = RevisionBasis::Evidence(nous_core::EvidenceRef {
        epistemic_relation: None,
        occurrence_id: occurrence,
        locator: nous_core::EvidenceLocator::WholeOccurrence,
        basis_role: BasisRole::Direct,
    });
    let first = memory
        .create_episode(EpisodeInput {
            operation_id: OperationId::new(),
            subject,
            track_key: "manual".into(),
            title: Some("Manual evidence".into()),
            parent_episode_revision_id: None,
            experience_time: TemporalExtent::Unknown,
            boundary_explanation: "Unbound observation.".into(),
            producer_signature_id: None,
            members: vec![EpisodeMemberInput {
                reference: CognitiveRef::Occurrence(occurrence),
                role: "evidence".into(),
            }],
            basis: vec![evidence],
        })
        .await
        .unwrap();
    let committed = memory
        .form_memory(consolidation_memory(&first))
        .await
        .unwrap();
    let reference = CognitiveRef::MemoryRevision(committed.revision.memory_revision_id);
    clock.advance_by(subject, Duration::seconds(1)).unwrap();
    let second = memory
        .create_episode(EpisodeInput {
            operation_id: OperationId::new(),
            subject,
            track_key: "manual".into(),
            title: Some("Cognitive continuation".into()),
            parent_episode_revision_id: None,
            experience_time: TemporalExtent::Unknown,
            boundary_explanation: "Memory member.".into(),
            producer_signature_id: None,
            members: vec![EpisodeMemberInput {
                reference: reference.clone(),
                role: "context".into(),
            }],
            basis: vec![RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: reference,
                basis_role: BasisRole::Direct,
            })],
        })
        .await
        .unwrap();
    (occurrence, first, second)
}

async fn assert_manual_consolidation_plans(
    rt: &NousRuntime,
    clock: &ManualCognitiveClock,
    subject: nous_core::SubjectId,
    first: &EpisodeView,
    second: &EpisodeView,
) {
    use nous_protocol::{
        kernel as k, kernel::kernel_maintenance_service_server::KernelMaintenanceService,
    };
    clock.advance_by(subject, Duration::seconds(300)).unwrap();
    let service = nous_kernel::transport::KernelService(rt.clone());
    let claims = service
        .claim_maintenance(tonic::Request::new(k::ClaimMaintenanceRequest {
            subject_id: subject.0.to_string(),
            allowed_kinds: vec!["memory_consolidate".into()],
            limit: 8,
            lease_seconds: 60,
            model_execution_digest: None,
        }))
        .await
        .unwrap()
        .into_inner();
    for episode in [&first, &second] {
        let claimed = claims
            .needs
            .iter()
            .find(|need| need.scope_ref == episode.revision.episode_revision_id.0.to_string())
            .unwrap();
        let plan = service
            .plan_maintenance(tonic::Request::new(k::PlanMaintenanceRequest {
                claimed: Some(claimed.clone()),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(plan.status, "ready");
        assert_eq!(plan.provenance_roots.len(), 1);
        if episode.object.episode_id == first.object.episode_id {
            assert_eq!(plan.members.len(), 1);
            assert_eq!(plan.members[0].text, "durable experience");
            assert!(plan.members[0].session_id.is_none());
            assert!(plan.members[0].recorded_seq.is_none());
        }
    }
}

#[tokio::test]
async fn manual_episode_planning_and_complete_journal_revalidation() {
    use nous_core::{BasisRole, CognitionDependency, CognitiveRef, OperationId, RevisionBasis};
    use nous_memory::{EpisodeMemberInput, JournalInput, JournalPoint, JournalPointRole};
    let (root, url, _postgres) = database().await;
    let clock = Arc::new(ManualCognitiveClock::new(Utc::now().trunc_subsecs(6)));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let (occurrence, first, second) = manual_episode_sources(&rt, &clock, subject).await;
    assert_manual_consolidation_plans(&rt, &clock, subject, &first, &second).await;
    let memory = rt.require_memory().unwrap();
    let journal = memory
        .commit_journal(JournalInput {
            operation_id: OperationId::new(),
            subject,
            expected_authority_seq: rt.store.authority_seq(subject).await.unwrap(),
            target: None,
            sources: [&first, &second]
                .into_iter()
                .map(|episode| EpisodePartitionSource {
                    revision: episode.revision.episode_revision_id,
                    expected_epoch: episode.object.object_epoch,
                })
                .collect(),
            title: None,
            narrative: "Manual sources.".into(),
            points: vec![JournalPoint {
                role: JournalPointRole::Summary,
                text: "Supported continuation.".into(),
                basis: vec![RevisionBasis::CognitionDependency(CognitionDependency {
                    epistemic_relation: None,
                    target_revision: CognitiveRef::EpisodeRevision(
                        second.revision.episode_revision_id,
                    ),
                    basis_role: BasisRole::Direct,
                })],
            }],
            producer: None,
        })
        .await
        .unwrap();
    let consolidation = memory
        .plan_consolidation_scope(
            subject,
            "journal_revision",
            &journal.revision.journal_revision_id.0.to_string(),
        )
        .await
        .unwrap();
    assert_eq!(consolidation.status, "ready");
    assert_eq!(consolidation.occurrences, vec![occurrence]);
    let revised = memory
        .revise_episode(nous_memory::ReviseEpisodeInput {
            operation_id: OperationId::new(),
            subject,
            episode_id: second.object.episode_id,
            expected_object_epoch: second.object.object_epoch,
            intent: "reinterpret".into(),
            title: Some("Revised continuation".into()),
            parent_episode_revision_id: None,
            experience_time: second.revision.experience_time.clone(),
            boundary_explanation: second.revision.boundary_explanation.clone(),
            producer_signature_id: None,
            members: second
                .members
                .iter()
                .map(|member| EpisodeMemberInput {
                    reference: member.reference.clone(),
                    role: member.role.clone(),
                })
                .collect(),
            basis: second.basis.clone(),
        })
        .await
        .unwrap();
    let scope = journal.object.journal_id.0.to_string();
    let waiting = memory
        .plan_journal_review(subject, "journal_revalidate", &scope)
        .await
        .unwrap();
    assert_eq!(waiting.episodes.len(), 1);
    assert_eq!(
        waiting.episodes[0].object.episode_id,
        first.object.episode_id
    );
    assert_eq!(waiting.status, "deferred");
    assert_eq!(waiting.problem.as_deref(), Some("journal_sources_settling"));
    clock.advance_by(subject, Duration::seconds(300)).unwrap();
    let ready = memory
        .plan_journal_review(subject, "journal_revalidate", &scope)
        .await
        .unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.episodes.len(), 2);
    assert!(ready.episodes.iter().any(
        |episode| episode.revision.episode_revision_id == revised.revision.episode_revision_id
    ));
    assert_eq!(ready.occurrences, vec![occurrence]);
    let consolidation = memory
        .plan_consolidation_scope(
            subject,
            "journal_revision",
            &journal.revision.journal_revision_id.0.to_string(),
        )
        .await
        .unwrap();
    assert_eq!(consolidation.status, "obsolete");
}
