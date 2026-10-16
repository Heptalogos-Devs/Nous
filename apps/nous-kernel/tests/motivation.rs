// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{Duration, Utc};
use nous_core::*;
use nous_motivation::*;
use nous_runtime::{DomainStamp, DomainTemporalExtent};
use test_support::{database, observation, open_runtime};
fn basis(id: OccurrenceId) -> RevisionBasis {
    RevisionBasis::Evidence(EvidenceRef {
        occurrence_id: id,
        locator: EvidenceLocator::WholeOccurrence,
        basis_role: BasisRole::Direct,
        epistemic_relation: None,
    })
}
fn matter(key: &str, id: OccurrenceId, condition: Option<DesiredCondition>) -> MotivationMatter {
    MotivationMatter {
        matter_key: key.into(),
        statement: format!("Subject wishes {key}"),
        scope: MotivationScope {
            description: "actual personal study".into(),
            ..Default::default()
        },
        basis: vec![basis(id)],
        valid_time: TemporalExtent::Unknown,
        producer: None,
        links: vec![],
        condition,
    }
}
fn condition(
    expression: Criterion,
    form: TemporalForm,
    start: chrono::DateTime<Utc>,
    end: chrono::DateTime<Utc>,
) -> DesiredCondition {
    DesiredCondition {
        expression,
        temporal_form: form,
        window: DomainTemporalExtent {
            time_domain_ref: "time:real".into(),
            extent: TemporalExtent::Interval {
                start: Some(start),
                end: Some(end),
            },
        },
        coverage_requirement: "Actual external observation coverage; gaps remain unknown".into(),
        hold_seconds: None,
    }
}
#[tokio::test]
async fn actual_owner_evaluation_lifecycle_and_restart() {
    let (root, url, _pg) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    runtime
        .subjects
        .provision_capabilities(
            subject,
            OperationId::new(),
            nous_configuration::SubjectCapabilities {
                memory: true,
                self_cognition: false,
                social_cognition: false,
                motivation: true,
            },
        )
        .await
        .unwrap();
    let o = observation(&runtime, subject, "I want to understand a bounded topic")
        .await
        .occurrence
        .occurrence_id;
    let owner = runtime.require_motivation().unwrap();
    let wish = owner
        .adopt(AdoptMotivation {
            subject,
            operation_id: OperationId::new(),
            content: MotivationContent::Desire(matter("study", o, None)),
            reason: "actual expressed interest".into(),
        })
        .await
        .unwrap();
    let start = Utc::now() - Duration::minutes(1);
    let end = Utc::now() + Duration::minutes(1);
    let desired = owner
        .adopt(AdoptMotivation {
            subject,
            operation_id: OperationId::new(),
            content: MotivationContent::DesiredCondition(matter(
                "study-result",
                o,
                Some(condition(
                    Criterion::EventWitness {
                        version: 1,
                        event_type: "study_completed".into(),
                        target: Some("topic:one".into()),
                        activity: None,
                    },
                    TemporalForm::Achieve,
                    start,
                    end,
                )),
            )),
            reason: "bounded learning condition".into(),
        })
        .await
        .unwrap();
    let event=runtime.material.record_observation(nous_material::ObservationInput{subject,session:None,occurrence:nous_material::OccurrenceDescriptor{source_class:SourceClass::Tool,external_object_ref:None,occurred_time:TemporalExtent::Instant{at:Utc::now()},observed_at:Some(Utc::now()),conversation_ref:None,actor_entity_ref:None,context:serde_json::json!({"criterion_fact":CriterionFact::Event{version:1,event_type:"study_completed".into(),target:Some("topic:one".into()),activity:None,at:DomainStamp{time_domain_ref:"time:real".into(),at:Utc::now(),recorded_at:Utc::now()}}})},material:nous_material::ObservationMaterial::InlineText{text:"actual successful tool result".into(),media_type:"text/plain".into()},runtime:Default::default(),entities:vec![]}).await.unwrap().occurrence.occurrence_id;
    let reader = OfficialCriterionReader {
        material: runtime.material.clone(),
        social: runtime.social_cognition.clone(),
    };
    let eval = EvaluateCondition {
        subject,
        operation_id: OperationId::new(),
        condition_revision: desired.reference(),
        scope: MotivationScope::default(),
        source_read: SourceReadScope {
            subject_id: Some(subject),
            source_refs: vec![CognitiveRef::Occurrence(o), CognitiveRef::Occurrence(event)],
        },
        evidence_refs: vec![CognitiveRef::Occurrence(event)],
    };
    let result = owner.evaluate(eval.clone(), &reader).await.unwrap();
    assert_eq!(result.verdict, ConditionVerdict::Satisfied);
    assert!(result.final_conclusion);
    assert_eq!(result.coverage, "limited");
    assert_eq!(
        owner
            .evaluate(eval.clone(), &reader)
            .await
            .unwrap()
            .evaluation_id,
        result.evaluation_id
    );
    let mut denied = eval.clone();
    denied.operation_id = OperationId::new();
    denied.source_read.source_refs = vec![];
    let denied = owner.evaluate(denied, &reader).await.unwrap();
    assert_eq!(denied.verdict, ConditionVerdict::Unknown);
    assert_eq!(denied.applicable, None);
    assert!(denied.leaves.is_empty());
    // A closed QQ-like stream without actual continuous coverage cannot establish Avoid.
    let avoid = owner
        .adopt(AdoptMotivation {
            subject,
            operation_id: OperationId::new(),
            content: MotivationContent::DesiredCondition(matter(
                "avoid-with-gap",
                o,
                Some(condition(
                    Criterion::EventWitness {
                        version: 1,
                        event_type: "opposed".into(),
                        target: None,
                        activity: None,
                    },
                    TemporalForm::Avoid,
                    start,
                    Utc::now() - Duration::seconds(1),
                )),
            )),
            reason: "actual concern".into(),
        })
        .await
        .unwrap();
    let gap = owner
        .evaluate(
            EvaluateCondition {
                condition_revision: avoid.reference(),
                operation_id: OperationId::new(),
                evidence_refs: vec![],
                ..eval.clone()
            },
            &reader,
        )
        .await
        .unwrap();
    assert_eq!(gap.verdict, ConditionVerdict::Unknown);
    assert!(!gap.final_conclusion);
    // Owner state has complete Authority history, unlike external QQ observations.
    let from = Utc::now();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let through = Utc::now();
    let maintained = owner
        .adopt(AdoptMotivation {
            subject,
            operation_id: OperationId::new(),
            content: MotivationContent::DesiredCondition(matter(
                "maintain-owner",
                o,
                Some(condition(
                    Criterion::OwnerState {
                        version: 1,
                        reference: wish.reference(),
                        state: "accepted".into(),
                    },
                    TemporalForm::Maintain,
                    from,
                    through,
                )),
            )),
            reason: "bounded complete owner history".into(),
        })
        .await
        .unwrap();
    let held = owner
        .evaluate(
            EvaluateCondition {
                condition_revision: maintained.reference(),
                operation_id: OperationId::new(),
                evidence_refs: vec![],
                ..eval.clone()
            },
            &reader,
        )
        .await
        .unwrap();
    assert_eq!(held.verdict, ConditionVerdict::Satisfied);
    assert_eq!(held.coverage, "complete");
    assert!(
        owner
            .revise(ReviseMotivation {
                subject,
                operation_id: OperationId::new(),
                object_id: wish.object_id,
                expected_revision: wish.revision_id,
                expected_epoch: 99,
                content: wish.content.clone(),
                reason: "stale".into()
            })
            .await
            .is_err()
    );
    let withdrawn = owner
        .lifecycle(
            subject,
            OperationId::new(),
            wish.object_reference(),
            wish.object_epoch,
            "withdraw",
        )
        .await
        .unwrap();
    assert_eq!(withdrawn.acceptance_state, "withdrawn");
    let saved_id = result.evaluation_id;
    drop(runtime);
    let restored = open_runtime(&url, &root).await;
    assert_eq!(
        restored
            .require_motivation()
            .unwrap()
            .evaluation(subject, saved_id)
            .await
            .unwrap()
            .verdict,
        ConditionVerdict::Satisfied
    );
}
