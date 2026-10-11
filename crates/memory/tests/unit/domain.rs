// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

fn evidence(occurrence: OccurrenceId) -> RevisionBasis {
    RevisionBasis::Evidence(EvidenceRef {
        epistemic_relation: None,
        occurrence_id: occurrence,
        locator: EvidenceLocator::WholeOccurrence,
        basis_role: BasisRole::Direct,
    })
}

#[test]
fn grounded_formation_binds_its_declared_occurrence() {
    let occurrence = OccurrenceId::new();
    let mut input = ExplicitMemoryInput {
        producer: None,
        operation_id: OperationId::new(),
        subject: SubjectId::new(),
        cognitive_role: CognitiveRole::Declarative,
        formation_mode: FormationMode::Grounded,
        grounding_occurrence_id: Some(occurrence),
        semantic_role: "fact".into(),
        representation_text: "bounded fact".into(),
        title: None,
        basis: vec![evidence(occurrence)],
        aboutness: Vec::new(),
        tags: Vec::new(),
        valid_time: TemporalExtent::Unknown,

        epistemic_class: EpistemicClass::Reported,
    };
    assert!(input.validate().is_ok());
    input.grounding_occurrence_id = Some(OccurrenceId::new());
    assert!(input.validate().is_err());
}

#[test]
fn synthesized_requires_two_inputs() {
    let input = ExplicitMemoryInput {
        producer: None,
        operation_id: OperationId::new(),
        subject: SubjectId::new(),
        cognitive_role: CognitiveRole::Declarative,
        formation_mode: FormationMode::Synthesized,
        grounding_occurrence_id: None,
        semantic_role: "fact".into(),
        representation_text: "bounded fact".into(),
        title: None,
        basis: vec![evidence(OccurrenceId::new())],
        aboutness: Vec::new(),
        tags: Vec::new(),
        valid_time: TemporalExtent::Unknown,

        epistemic_class: EpistemicClass::Inferred,
    };
    assert!(input.validate().is_err());
}

#[test]
fn provenance_relation_is_conservative_for_unknown_encounters() {
    let known = EvidenceRoot {
        root_key: "artifact:a".into(),
        certainty: EvidenceRootCertainty::Known,
    };
    let same = known.clone();
    assert_eq!(
        dependency_relation(std::slice::from_ref(&known), std::slice::from_ref(&same)),
        DependencyRelation::SameRoot
    );
    assert_eq!(
        dependency_relation(
            std::slice::from_ref(&known),
            &[EvidenceRoot {
                root_key: "artifact:b".into(),
                certainty: EvidenceRootCertainty::Known,
            }]
        ),
        DependencyRelation::Independent
    );
    assert_eq!(
        dependency_relation(
            &[EvidenceRoot {
                root_key: "occurrence:a".into(),
                certainty: EvidenceRootCertainty::OccurrenceOnly,
            }],
            &[EvidenceRoot {
                root_key: "occurrence:b".into(),
                certainty: EvidenceRootCertainty::OccurrenceOnly,
            }]
        ),
        DependencyRelation::UnknownDependency
    );
}
