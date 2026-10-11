// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_kernel::NousRuntime;
use nous_memory::CognitionView;
use nous_runtime::ManualCognitiveClock;
use std::sync::Arc;
use test_support::{database, read_mutation_cognition};

use test_support::longitudinal::{
    consolidation_memory, consolidation_producer, consolidation_schema, journal_source_episode,
    runtime_with_clock_serving,
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
    let schema_result = owner.create_schema(schema_input.clone()).await.unwrap();
    let schema = match read_mutation_cognition(owner, subject, &schema_result).await {
        CognitionView::CognitiveSchema(value) => value,
        _ => panic!("schema mutation result should materialize as CognitiveSchema"),
    };
    let mut invalid = consolidation_schema(&episode);
    invalid.content.boundary_definition.clear();
    assert!(owner.create_schema(invalid).await.is_err());
    assert_eq!(
        owner
            .form_memory(first_input.clone())
            .await
            .unwrap()
            .revision_id,
        first.revision_id
    );
    owner
        .link_revisions(
            subject,
            OperationId::new(),
            nous_core::MemoryRevisionId(first.revision_id.unwrap()),
            nous_core::MemoryRevisionId(second.revision_id.unwrap()),
            nous_memory::MemoryRelation::Elaborates,
        )
        .await
        .unwrap();
    let results = [
        Some(nous_core::CognitiveRef::MemoryRevision(
            nous_core::MemoryRevisionId(first.revision_id.unwrap()),
        )),
        Some(nous_core::CognitiveRef::MemoryRevision(
            nous_core::MemoryRevisionId(second.revision_id.unwrap()),
        )),
        Some(nous_core::CognitiveRef::CognitiveSchemaRevision(
            nous_core::CognitiveSchemaRevisionId(schema_result.revision_id.unwrap()),
        )),
    ];
    clock.advance_by(subject, Duration::days(1)).unwrap();
    assert_consolidation_context(&rt, subject, &results).await;
    let revised_input = nous_memory::ReviseSchemaInput {
        operation_id: OperationId::new(),
        subject,
        schema_id: schema.header.schema_id(),
        expected_object_epoch: schema.header.object_epoch,
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
    let revised_result = owner.revise_schema(revised_input.clone()).await.unwrap();
    let revised = match read_mutation_cognition(owner, subject, &revised_result).await {
        CognitionView::CognitiveSchema(value) => value,
        _ => panic!("schema mutation result should materialize as CognitiveSchema"),
    };
    assert!(revised.revision.header.producer_signature_id.is_some());
    assert_ne!(
        revised.revision.header.revision_id(),
        schema.revision.header.revision_id()
    );
    assert_eq!(
        owner.create_schema(schema_input).await.unwrap().revision_id,
        schema_result.revision_id
    );
    let mut next = revised_input.clone();
    next.operation_id = OperationId::new();
    next.expected_object_epoch = revised.header.object_epoch;
    owner.revise_schema(next).await.unwrap();
    assert_eq!(
        owner.revise_schema(revised_input).await.unwrap(),
        revised_result
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
            .all(|candidate| candidate.text.chars().count() <= 64)
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
