// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::Duration;
use chrono::Utc;
use nous_core::{CognitiveRef, Cue, QueryTarget, TemporalExtent, TextCue, TimePredicate};
use test_support::query::{query, subject};
use test_support::{database, open_runtime, open_runtime_with_serving};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real evidence cohort verifies past/future occurrence and region filters through finalization"
)]
async fn evidence_time_constraints_filter_occurrences_and_regions_through_finalization() {
    use nous_material::{
        ObservationInput, ObservationMaterial, OccurrenceDescriptor, RuntimeDirective,
    };
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = subject(&runtime).await;
    let now = chrono::SubsecRound::trunc_subsecs(Utc::now(), 6);
    let mut observations = Vec::new();
    for (time, text) in [
        (now - Duration::days(2), "chronicle historical approval"),
        (now + Duration::days(2), "chronicle future approval"),
    ] {
        observations.push(
            runtime
                .material
                .record_observation(ObservationInput {
                    subject,
                    session: None,
                    occurrence: OccurrenceDescriptor {
                        source_class: nous_core::SourceClass::Message,
                        external_object_ref: None,
                        occurred_time: TemporalExtent::Instant { at: time },
                        observed_at: Some(now),
                        conversation_ref: None,
                        actor_entity_ref: None,
                        context: serde_json::json!({}),
                    },
                    material: ObservationMaterial::InlineText {
                        text: text.into(),
                        media_type: "text/plain".into(),
                    },
                    entities: Vec::new(),
                    runtime: RuntimeDirective::default(),
                })
                .await
                .unwrap(),
        );
    }
    let mut input = query(subject);
    input.projection.domains = vec![nous_core::ResultDomain::Evidence];
    input.expression.cues.push(Cue::Text(TextCue {
        text: "chronicle approval".into(),
    }));
    input.expression.constraints.occurred = Some(TimePredicate::Range {
        start: None,
        end: Some(now),
    });
    let execution = runtime
        .execute_query(input.clone(), Some(32))
        .await
        .unwrap();
    let past = CognitiveRef::Occurrence(observations[0].occurrence.occurrence_id);
    let future = CognitiveRef::Occurrence(observations[1].occurrence.occurrence_id);
    input.expression.constraints.occurred = Some(TimePredicate::Point {
        at: now - Duration::days(2),
    });
    let point = runtime.query(input).await.unwrap();
    assert!(point.results.iter().any(|hit| {
        hit.reference == past
            && hit
                .match_evidence
                .families
                .contains(&nous_core::EvidenceFamily::Temporal)
    }));
    assert!(!point.results.iter().any(|hit| hit.reference == future));
    let mut fine_range = query(subject);
    fine_range.projection.domains = vec![nous_core::ResultDomain::Evidence];
    let at = now - Duration::days(2);
    fine_range.expression.constraints.occurred = Some(TimePredicate::Range {
        start: Some(at - Duration::nanoseconds(1)),
        end: Some(at + Duration::nanoseconds(1)),
    });
    let result = runtime.query(fine_range).await.unwrap();
    assert!(result.results.iter().any(|hit| hit.reference == past));
    let past_region = CognitiveRef::SourceRegion(
        observations[0]
            .source_region
            .as_ref()
            .unwrap()
            .source_region_id,
    );
    let future_region = CognitiveRef::SourceRegion(
        observations[1]
            .source_region
            .as_ref()
            .unwrap()
            .source_region_id,
    );
    assert!(
        execution
            .result
            .results
            .iter()
            .any(|hit| hit.reference == past_region)
    );
    assert!(
        !execution
            .result
            .results
            .iter()
            .any(|hit| hit.reference == future_region)
    );
    assert!(
        execution
            .result
            .results
            .iter()
            .any(|hit| hit.reference == past)
    );
    assert!(
        !execution
            .result
            .results
            .iter()
            .any(|hit| hit.reference == future)
    );
    assert!(
        execution
            .result
            .results
            .iter()
            .filter(|hit| matches!(
                hit.reference,
                CognitiveRef::Occurrence(_) | CognitiveRef::SourceRegion(_)
            ))
            .all(|hit| !hit.freshness.occurred.is_empty()
                && hit
                    .freshness
                    .occurred
                    .iter()
                    .all(|time| TimePredicate::Range {
                        start: None,
                        end: Some(now)
                    }
                    .matches_extent(time)))
    );
    let (_, ticket) = runtime
        .cognition
        .retain_query(execution, test_support::query_lease())
        .unwrap();
    let result = runtime
        .cognition
        .finalize_query(
            subject,
            ticket.unwrap(),
            Vec::new(),
            Vec::new(),
            nous_runtime::CognitiveContributors {
                material: Some(&runtime.material),
                shared: None,
                memory: runtime
                    .memory
                    .as_ref()
                    .map(|owner| owner as &dyn nous_runtime::CognitiveContributor),
            },
        )
        .await
        .unwrap();
    assert!(result.results.iter().any(|hit| hit.reference == past));
    assert!(!result.results.iter().any(|hit| hit.reference == future));
    assert!(
        !result
            .results
            .iter()
            .any(|hit| hit.reference == future_region)
    );
}

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one real Material cohort protects joint source axes and derived owner timestamps"
)]
async fn material_query_keeps_joint_observation_axes_and_derived_formation_time() {
    use nous_material::{
        DerivationInput, DerivedRepresentation, ObservationInput, ObservationMaterial,
        OccurrenceDescriptor, RuntimeDirective,
    };
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let now = Utc::now();
    let early = now - Duration::days(2);
    let late = now - Duration::days(1);
    let cutoff = early + Duration::hours(12);
    let mut observations = Vec::new();
    for (occurred, observed) in [(early, late), (late, early)] {
        observations.push(
            runtime
                .material
                .record_observation(ObservationInput {
                    subject,
                    session: None,
                    occurrence: OccurrenceDescriptor {
                        source_class: nous_core::SourceClass::File,
                        external_object_ref: None,
                        occurred_time: TemporalExtent::Instant { at: occurred },
                        observed_at: Some(observed),
                        conversation_ref: None,
                        actor_entity_ref: None,
                        context: serde_json::json!({}),
                    },
                    material: ObservationMaterial::InlineText {
                        text: "the same jointly observed source".into(),
                        media_type: "text/plain".into(),
                    },
                    entities: Vec::new(),
                    runtime: RuntimeDirective::default(),
                })
                .await
                .unwrap(),
        );
    }
    assert_eq!(
        observations[0].artifact.as_ref().unwrap().artifact_id,
        observations[1].artifact.as_ref().unwrap().artifact_id
    );
    let source = CognitiveRef::SourceRegion(
        observations[0]
            .source_region
            .as_ref()
            .unwrap()
            .source_region_id,
    );
    let derived = runtime
        .material
        .persist_derived_representation(DerivedRepresentation {
            derived_representation_id: nous_core::DerivedRepresentationId::new(),
            subject_id: subject,
            inputs: vec![DerivationInput {
                ordinal: 0,
                reference: source.clone(),
                role: "source".into(),
            }],
            strategy: "deterministic-test".into(),
            representation_kind: nous_core::RepresentationKind::ExtractedText,
            producer: nous_core::ProducerSignature {
                model_role: None,
                model_profile: None,
                execution_profile: None,
                inference_controls_digest: None,
                role_policy_digest: None,
                prompt_id: None,
                prompt_digest: None,

                signature_hash: String::new(),
                provider_class: "local-test".into(),
                operation: nous_core::CapabilityOperation::TextInterpretation,
                implementation: "joint-time-test".into(),
                model_identity: None,
                model_revision: None,
                output_schema_digest: None,
                preprocessing_identity: "utf8".into(),
                preprocessing_revision: "1".into(),
                config_digest: "test".into(),
            },
            revision: 1,
            payload_text: Some("the same jointly observed source".into()),
            payload_json: None,
            payload_artifact_id: None,
            quality: serde_json::json!({}),
            created_at: early,
            supersedes: None,
        })
        .await
        .unwrap();
    let derived_ref = CognitiveRef::DerivedRepresentation(derived.derived_representation_id);
    for reference in [source, derived_ref.clone()] {
        let mut input = query(subject);
        input.projection.domains = vec![nous_core::ResultDomain::Evidence];
        input.expression.targets = vec![QueryTarget::Exact {
            reference: reference.clone(),
        }];
        input.expression.constraints.occurred = Some(TimePredicate::Range {
            start: None,
            end: Some(cutoff),
        });
        input.expression.constraints.observed = Some(TimePredicate::Range {
            start: None,
            end: Some(cutoff),
        });
        let result = runtime.execute_query(input.clone(), None).await.unwrap();
        assert!(
            result.result.results.is_empty(),
            "different observations cannot supply separate requested axes"
        );
        input.expression.constraints.observed = Some(TimePredicate::Range {
            start: Some(cutoff),
            end: Some(now),
        });
        let result = runtime.execute_query(input, None).await.unwrap();
        assert_eq!(result.result.results.len(), 1);
    }
    let mut temporal = query(subject);
    temporal.projection.domains = vec![nous_core::ResultDomain::Evidence];
    temporal.expression.constraints.occurred = Some(TimePredicate::Range {
        start: None,
        end: Some(cutoff),
    });
    temporal.expression.constraints.observed = Some(TimePredicate::Range {
        start: Some(cutoff),
        end: Some(now),
    });
    let result = runtime.execute_query(temporal, None).await.unwrap();
    assert_eq!(result.result.results.len(), 1);
    assert_eq!(
        result.result.results[0].reference,
        CognitiveRef::Occurrence(observations[0].occurrence.occurrence_id)
    );
    let mut input = query(subject);
    input.projection.domains = vec![nous_core::ResultDomain::Evidence];
    input.expression.targets = vec![QueryTarget::Exact {
        reference: derived_ref,
    }];
    let result = runtime.execute_query(input.clone(), None).await.unwrap();
    let freshness = &result.result.results[0].freshness;
    assert!(freshness.formed_at.unwrap() >= now);
    assert!(freshness.observed_at.unwrap() < now);
    assert!(matches!(freshness.valid_time, TemporalExtent::Unknown));
    input.expression.constraints.formed = Some(TimePredicate::Range {
        start: None,
        end: Some(now),
    });
    assert!(
        runtime
            .execute_query(input, None)
            .await
            .unwrap()
            .result
            .results
            .is_empty()
    );
}
