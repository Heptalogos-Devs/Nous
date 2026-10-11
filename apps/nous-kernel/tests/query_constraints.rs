// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{
    CognitiveQueryExpr, CognitiveRef, Cue, EntityRef, OperationId, QueryOperation, QueryTarget,
    TextCue,
};
use nous_memory::CognitiveRole;
use test_support::query::{query, subject};
use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "shared entity cohort verifies Boolean scope and soft preference eligibility without duplicate database setup"
)]
async fn entity_lane_uses_aboutness_and_multi_value_include() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let alice = EntityRef::new("entity:alice").expect("entity");
    let first_observation = observation(&runtime, subject, "Alice declarative").await;
    let second_observation = observation(&runtime, subject, "Alice experiential").await;
    let mut first = form_input(
        subject,
        first_observation.occurrence.occurrence_id,
        OperationId::new(),
        "declarative about Alice",
    );
    first.aboutness = vec![alice.clone()];
    first.cognitive_role = CognitiveRole::Declarative;
    let mut second = form_input(
        subject,
        second_observation.occurrence.occurrence_id,
        OperationId::new(),
        "experiential about Alice",
    );
    second.aboutness = vec![alice.clone()];
    second.cognitive_role = CognitiveRole::Experiential;
    let first = runtime
        .require_memory()
        .unwrap()
        .form_memory(first)
        .await
        .expect("first memory");
    let second = runtime
        .require_memory()
        .unwrap()
        .form_memory(second)
        .await
        .expect("second memory");
    let mut domain_fenced = query(subject);
    domain_fenced.projection.domains = vec![nous_core::ResultDomain::Memory];
    domain_fenced.expression = CognitiveQueryExpr {
        operation: QueryOperation::Any,
        children: vec![
            CognitiveQueryExpr {
                targets: vec![QueryTarget::Exact {
                    reference: CognitiveRef::Artifact(
                        first_observation.artifact.as_ref().unwrap().artifact_id,
                    ),
                }],
                ..Default::default()
            },
            CognitiveQueryExpr {
                cues: vec![Cue::Text(TextCue {
                    text: "Recall memory within requested domain".into(),
                })],
                targets: vec![QueryTarget::Exact {
                    reference: CognitiveRef::MemoryRevision(first.revision.memory_revision_id),
                }],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let fenced = runtime
        .query(domain_fenced)
        .await
        .expect("parent domain fences exact evidence");
    assert!(
        fenced
            .results
            .iter()
            .all(|hit| !matches!(hit.reference, CognitiveRef::Artifact(_)))
    );
    assert!(fenced.results.iter().any(
        |hit| hit.reference == CognitiveRef::MemoryRevision(first.revision.memory_revision_id)
    ));
    let mut request = query(subject);
    request.expression.cues = vec![
        Cue::Text(nous_core::TextCue {
            text: "Recall cognition for this entity".into(),
        }),
        Cue::Entity(nous_core::EntityCue { entity_ref: alice }),
    ];
    request.expression.constraints.cognitive_roles_include =
        vec!["declarative".into(), "experiential".into()];
    let result = runtime.query(request.clone()).await.expect("entity query");
    let references = result
        .results
        .into_iter()
        .map(|hit| hit.reference)
        .collect::<Vec<_>>();
    assert!(references.contains(&CognitiveRef::MemoryRevision(
        first.revision.memory_revision_id,
    )));
    assert!(references.contains(&CognitiveRef::MemoryRevision(
        second.revision.memory_revision_id,
    )));
    let mut declarative = request.expression.clone();
    declarative.constraints.cognitive_roles_include = vec!["declarative".into()];
    let mut experiential = request.expression.clone();
    experiential.constraints.cognitive_roles_include = vec!["experiential".into()];
    let mut tree = query(subject);
    tree.projection.domains = vec![nous_core::ResultDomain::Memory];
    tree.expression = CognitiveQueryExpr {
        operation: QueryOperation::All,
        children: vec![declarative, experiential],
        ..Default::default()
    };
    assert!(
        runtime
            .query(tree.clone())
            .await
            .expect("scoped intersection")
            .results
            .is_empty()
    );
    tree.expression.operation = QueryOperation::Any;
    assert_eq!(
        runtime
            .query(tree.clone())
            .await
            .expect("scoped union")
            .results
            .len(),
        2
    );
    tree.expression.constraints.cognitive_roles_include = vec!["declarative".into()];
    let narrowed = runtime.query(tree).await.expect("parent narrowing");
    assert_eq!(narrowed.results.len(), 1);
    assert_eq!(
        narrowed.results[0].reference,
        CognitiveRef::MemoryRevision(first.revision.memory_revision_id)
    );
    let mut preferred = request.clone();
    preferred.expression.preferences = vec![nous_core::QueryPreference {
        negative: false,
        operand: nous_core::PreferenceOperand::Cue(Cue::Text(TextCue {
            text: "experiential".into(),
        })),
    }];
    let positive = runtime
        .query(preferred.clone())
        .await
        .expect("positive preference");
    assert_eq!(positive.results.len(), 2);
    assert_eq!(
        positive.results[0].reference,
        CognitiveRef::MemoryRevision(second.revision.memory_revision_id)
    );
    assert!(positive.results[0].match_evidence.preference_score > 0.0);
    preferred.expression.preferences[0].negative = true;
    let negative = runtime
        .query(preferred.clone())
        .await
        .expect("negative preference");
    assert_eq!(
        negative.results[0].reference,
        CognitiveRef::MemoryRevision(first.revision.memory_revision_id)
    );
    preferred.expression.preferences = vec![nous_core::QueryPreference {
        negative: false,
        operand: nous_core::PreferenceOperand::Recent(nous_core::TimeAxis::Valid),
    }];
    assert!(
        runtime
            .query(preferred.clone())
            .await
            .expect("unknown valid axis")
            .results
            .iter()
            .all(|hit| hit.match_evidence.preference_score == 0.0)
    );
    preferred.expression.preferences = vec![
        nous_core::QueryPreference {
            negative: false,
            operand: nous_core::PreferenceOperand::Cue(Cue::Text(TextCue {
                text: "experiential".into()
            }))
        };
        16
    ];
    let capped = runtime
        .query(preferred.clone())
        .await
        .expect("bounded preference");
    assert_eq!(capped.results[0].match_evidence.preference_score, 0.06);
    preferred.expression.constraints.cognitive_roles_include = vec!["declarative".into()];
    assert_eq!(
        runtime
            .query(preferred)
            .await
            .expect("preference cannot bypass constraints")
            .results
            .len(),
        1
    );
}
