use nous_domain::*;
fn support(source: EvidenceSource) -> EvidenceLink {
    EvidenceLink {
        source,
        polarity: EvidencePolarity::Support,
        relation: Some(EpistemicRelation::Supports),
        explanation: "actual source input".into(),
    }
}
fn material() -> EvidenceSource {
    EvidenceSource::Material {
        material_id: MaterialId::new(),
    }
}
fn stance() -> EpistemicStance {
    EpistemicStance {
        class: EpistemicClass::Reported,
        attribution: Some("source record".into()),
        confidence: Some(0.7),
    }
}
fn concept(label: &str) -> CognitiveContent {
    CognitiveContent::Concept(ConceptContent {
        label: label.into(),
        description: "supported concept meaning".into(),
        aliases: vec![],
        canonical: None,
        members: vec![],
        lineage: vec![],
    })
}
fn times() -> CognitiveTimes {
    CognitiveTimes {
        event: None,
        observation: None,
        formation: TimePoint {
            domain: "utc".into(),
            coordinate: 1,
        },
        validity: None,
        recorded: TimePoint {
            domain: "utc".into(),
            coordinate: 1,
        },
    }
}
#[test]
fn formation_roles_stance_and_exact_support_are_separate() {
    let observation = ObservationId::new();
    let mut content = MemorySemantics {
        memory_id: MemoryId::new(),
        memory_revision: 1,
        cognitive_role: CognitiveRole::Declarative,
        formation_mode: FormationMode::Grounded,
        epistemic: stance(),
        grounding_observation: Some(observation.clone()),
        concepts: vec![],
        qualifications: vec!["reported rather than witnessed content".into()],
    };
    let source = support(EvidenceSource::Observation {
        observation_id: observation,
    });
    assert!(
        CognitiveContent::MemorySemantics(content.clone())
            .validate(std::slice::from_ref(&source))
            .is_ok()
    );
    assert!(
        CognitiveContent::MemorySemantics(content.clone())
            .validate(&[])
            .is_err()
    );
    content.formation_mode = FormationMode::Synthesized;
    content.grounding_observation = None;
    assert!(
        CognitiveContent::MemorySemantics(content.clone())
            .validate(std::slice::from_ref(&source))
            .is_err()
    );
    assert!(
        CognitiveContent::MemorySemantics(content)
            .validate(&[source, support(material())])
            .is_ok()
    );
    let malformed = serde_json::json!({"kind":"material","material_id":MaterialId::new(),"independent_roots":["claimed by caller"]});
    assert!(serde_json::from_value::<EvidenceSource>(malformed).is_err());
    assert!(
        CognitiveRef {
            kind: CognitiveKind::Concept,
            id: CognitiveId::new().to_string(),
            revision: 0
        }
        .validate()
        .is_err()
    );
}
#[test]
fn body_sources_preserve_counterevidence_context_and_all_exact_references() {
    let a = material();
    let b = material();
    let c = material();
    let concept_ref = CognitiveRef::structured(CognitiveKind::Concept, &CognitiveId::new(), 3);
    let body = CognitiveContent::Schema(SchemaContent {
        title: "group response pattern".into(),
        claim: "responds to narrow topics".into(),
        scope: SchemaScope {
            description: "large public groups".into(),
            about: vec![],
            concepts: vec![concept_ref.clone()],
            validity: None,
        },
        boundary: "private conversation is outside this pattern".into(),
        formation_mode: FormationMode::Grounded,
        epistemic: stance(),
        support: vec![a.clone()],
        counterexamples: vec![b.clone()],
        boundary_cases: vec![c.clone()],
        lineage: vec![],
    });
    body.validate(&[]).unwrap();
    let links = body.evidence_links();
    assert!(
        links
            .iter()
            .any(|link| link.source == a && link.polarity == EvidencePolarity::Support)
    );
    assert!(
        links
            .iter()
            .any(|link| link.source == b && link.polarity == EvidencePolarity::Contradict)
    );
    assert!(
        links
            .iter()
            .any(|link| link.source == c && link.polarity == EvidencePolarity::Context)
    );
    assert!(body.references().contains(&concept_ref));
    let mut object = StructuredObject {
        id: CognitiveId::new(),
        namespace_id: NamespaceId::new(),
        subject_id: SubjectId::new(),
        revision: 1,
        body,
        evidence: vec![],
        times: times(),
        producer_call_id: None,
        recorded_at: "2026-10-23T00:00:00Z".into(),
        commit_seq: 1,
        deleted: false,
    };
    object.validate().unwrap();
    assert_eq!(object.sources().len(), 4);
    object.body = concept("label");
    object.evidence = vec![support(material())];
    if let CognitiveContent::Concept(value) = &mut object.body {
        value.aliases = vec![" LABEL ".into()];
    }
    assert!(object.validate().is_err());
    if let CognitiveContent::Concept(value) = &mut object.body {
        value.aliases.clear();
        value.canonical = Some(CognitiveRef::structured(
            CognitiveKind::Concept,
            &object.id,
            1,
        ));
    }
    assert!(object.validate().is_err());
    let schema = schemars::schema_for!(StructuredObject);
    assert!(
        serde_json::to_value(schema)
            .unwrap()
            .to_string()
            .contains("MemorySemantics")
    );
}
#[test]
fn episode_member_order_and_journal_supported_perspective_have_rules() {
    let source = material();
    let episode = CognitiveRef::structured(CognitiveKind::Episode, &CognitiveId::new(), 1);
    let mut content = EpisodeContent {
        title: "an actual work session".into(),
        track: "task".into(),
        experience: TimeRange {
            domain: "utc".into(),
            start: Some(1),
            end: Some(2),
        },
        parent: None,
        members: vec![EpisodeMember {
            source: source.clone(),
            role: "experience".into(),
            ordinal: 1,
        }],
        boundary_explanation: "session ended".into(),
        lineage: vec![],
    };
    assert!(
        CognitiveContent::Episode(content.clone())
            .validate(&[])
            .is_err()
    );
    content.members[0].ordinal = 0;
    CognitiveContent::Episode(content).validate(&[]).unwrap();
    let mut journal = JournalContent {
        title: "day narrative".into(),
        narrative: "what was understood then".into(),
        period: TimeRange {
            domain: "utc".into(),
            start: Some(1),
            end: Some(2),
        },
        knowledge_cutoff: 4,
        perspective: EpistemicStance {
            class: EpistemicClass::Narrative,
            attribution: None,
            confidence: None,
        },
        points: vec![JournalPoint {
            role: JournalPointRole::Reflection,
            text: "a supported interpretation".into(),
            evidence: vec![],
        }],
        episodes: vec![episode.clone()],
        lineage: vec![],
    };
    assert!(
        CognitiveContent::Journal(journal.clone())
            .validate(&[])
            .is_err()
    );
    journal.points[0].evidence = vec![support(source)];
    let body = CognitiveContent::Journal(journal);
    body.validate(&[]).unwrap();
    assert!(body.references().contains(&episode));
}
#[test]
fn preallocated_slot_dependencies_bound_model_proposals_without_invented_ids() {
    let a = StructuredTargetSlot {
        key: "first".into(),
        kind: CognitiveKind::Concept,
        id: CognitiveId::new(),
        expected_revision: 0,
    };
    let b = StructuredTargetSlot {
        key: "second".into(),
        kind: CognitiveKind::Concept,
        id: CognitiveId::new(),
        expected_revision: 0,
    };
    let source = material();
    let evidence = vec![support(source.clone())];
    let mut dependent = concept("dependent concept");
    if let CognitiveContent::Concept(value) = &mut dependent {
        value.members = vec![a.output_reference().unwrap()];
    }
    let mut proposal = StructuredProposal {
        groups: vec![
            StructuredProposalGroup {
                key: "a".into(),
                depends_on: vec![],
                changes: vec![StructuredProposalChange {
                    slot_key: "first".into(),
                    body: concept("first concept"),
                    evidence: evidence.clone(),
                }],
            },
            StructuredProposalGroup {
                key: "b".into(),
                depends_on: vec!["a".into()],
                changes: vec![StructuredProposalChange {
                    slot_key: "second".into(),
                    body: dependent,
                    evidence,
                }],
            },
        ],
    };
    proposal
        .validate_against(&[a.clone(), b.clone()], std::slice::from_ref(&source))
        .unwrap();
    proposal.groups[1].depends_on.clear();
    assert!(proposal.validate_against(&[a, b], &[source]).is_err());
}
