// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn query(effort: CognitiveEffort) -> CognitiveQuery {
    CognitiveQuery {
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        subject: SubjectId::new(),
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            operation: QueryOperation::Atom,
            preferences: Vec::new(),
            children: Vec::new(),
            targets: Vec::new(),
            cues: vec![Cue::Text(TextCue {
                text: "query".into(),
            })],
            constraints: Default::default(),
        },
        exploration: ExplorationIntent::None,
        resources: Default::default(),
        result_need: Default::default(),
        effort,
        capabilities: Default::default(),
        diagnostics: DiagnosticsRequest::Summary,
    }
}

fn reference_plan(query: &CognitiveQuery) -> QueryPlan {
    QueryPlan::from_policy(
        query,
        &[],
        super::super::ConceptEnrichment::Off,
        super::super::RetrievalPolicy::reference(),
    )
    .expect("reference query plan")
}

#[test]
fn deep_text_does_not_enable_topology_without_explicit_intent() {
    assert!(!reference_plan(&query(CognitiveEffort::Deep)).expand_topology);
}

#[test]
fn validation_budget_is_the_frozen_reference_formula() {
    let mut value = query(CognitiveEffort::Normal);
    value.result_need.limit = 12;
    assert_eq!(reference_plan(&value).final_validation_budget, 48);
}

#[test]
fn exact_read_is_scoped_and_does_not_require_serving() {
    let mut value = query(CognitiveEffort::Deep);
    value.expression.targets.push(QueryTarget::Exact {
        reference: CognitiveRef::Memory(MemoryId::new()),
    });
    value.session = Some(SessionId::new());
    value.exploration = ExplorationIntent::BoundedAssociative;
    value.capabilities.residual_sensing = RequirementStrength::Required;
    let plan = reference_plan(&value);
    assert_eq!(plan.enabled_lanes, vec![EvidenceFamily::Exact]);
    assert!(!plan.expand_topology);
    assert!(!plan.sense_cues);
    assert_eq!(plan.serving_need(&value), ServingNeed::default());
    let atom = value.expression.clone();
    value.expression.operation = QueryOperation::Any;
    value.expression.cues.clear();
    value.expression.children = vec![atom.clone(), atom.clone()];
    // Empty child targets inherit the parent's exact target.
    value.expression.children[0].targets.clear();
    assert!(value.is_exact_read());
    assert_eq!(
        reference_plan(&value).enabled_lanes,
        vec![EvidenceFamily::Exact]
    );
    value.expression.targets.clear();
    assert!(!value.is_exact_read());
    let mixed = reference_plan(&value);
    assert!(mixed.enabled_lanes.contains(&EvidenceFamily::Exact));
    assert!(mixed.enabled_lanes.contains(&EvidenceFamily::Lexical));
}
