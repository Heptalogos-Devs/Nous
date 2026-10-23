use nous_app::App;
use nous_app::common::instance::InstancePaths;
use nous_domain::*;
fn times() -> CognitiveTimes {
    CognitiveTimes {
        event: None,
        observation: None,
        formation: TimePoint {
            domain: "unix_ms".into(),
            coordinate: 100,
        },
        validity: None,
        recorded: TimePoint {
            domain: "unix_ms".into(),
            coordinate: 100,
        },
    }
}
fn content(label: &str) -> ConceptContent {
    ConceptContent {
        label: label.into(),
        description: format!("meaning of {label}"),
        aliases: vec![],
        canonical: None,
        members: vec![],
        lineage: vec![],
    }
}
async fn create(
    app: &App,
    namespace: &NamespaceId,
    subject: &SubjectId,
    key: &str,
    body: CognitiveContent,
    evidence: &[EvidenceLink],
) -> StructuredObject {
    serde_json::from_value(
        app.create_structured(
            &CallContext::admin(),
            key,
            CreateStructuredRequest {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
                expected_revision: 0,
                body,
                evidence: evidence.to_vec(),
                times: times(),
                producer_call_id: None,
            },
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap()
}
fn expected(object: &StructuredObject) -> StructuredExpectation {
    StructuredExpectation {
        id: object.id.clone(),
        expected_revision: object.revision,
    }
}
#[tokio::test]
async fn exact_semantic_maintenance_replays_and_rolls_back_whole_lineage_operations() {
    let parent =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/instances/lineage-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(directory.path()).unwrap();
    paths.initialize().unwrap();
    let app = App::open(paths).await.unwrap();
    let actor = CallContext::admin();
    let namespace: Namespace = serde_json::from_value(
        app.create_namespace(&actor, "ns", "lineage")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let subject: Subject = serde_json::from_value(
        app.create_subject(
            &actor,
            "subject",
            &namespace.id,
            "reader",
            vec![Capability::Memory],
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let material: Material = serde_json::from_value(
        app.put_material(
            &actor,
            "material",
            &namespace.id,
            "actual source material",
            Some("test:lineage".into()),
            None,
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let first: Observation = serde_json::from_value(
        app.record_observation(
            &actor,
            "first-contact",
            &subject.id,
            &material.id,
            Some(TimePoint {
                domain: "fixture".into(),
                coordinate: 10,
            }),
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let second: Observation = serde_json::from_value(
        app.record_observation(
            &actor,
            "second-contact",
            &subject.id,
            &material.id,
            Some(TimePoint {
                domain: "fixture".into(),
                coordinate: 20,
            }),
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let evidence = vec![EvidenceLink {
        source: EvidenceSource::Material {
            material_id: material.id,
        },
        polarity: EvidencePolarity::Support,
        relation: None,
        explanation: "provided source".into(),
    }];
    let a = create(
        &app,
        &namespace.id,
        &subject.id,
        "concept-a",
        CognitiveContent::Concept(content("original concept")),
        &evidence,
    )
    .await;
    let b = create(
        &app,
        &namespace.id,
        &subject.id,
        "concept-b",
        CognitiveContent::Concept(content("survivor concept")),
        &evidence,
    )
    .await;
    let merge = MergeConceptsRequest {
        survivor: expected(&b),
        retired: vec![expected(&a)],
        evidence: evidence.clone(),
        reason: "same stable meaning".into(),
    };
    let receipt = app
        .merge_concepts(&actor, "merge", merge.clone())
        .await
        .unwrap();
    assert_eq!(
        receipt,
        app.merge_concepts(&actor, "merge", merge).await.unwrap()
    );
    let survivor: StructuredObject =
        serde_json::from_value(receipt.result["survivor"].clone()).unwrap();
    assert_eq!(survivor.id, b.id);
    assert_eq!(survivor.revision, 2);
    let retired = app.get_structured(&actor, &a.id, None).await.unwrap();
    assert_eq!(concept(&retired).unwrap().canonical, Some(b.reference()));
    assert!(
        concept(&survivor).unwrap().lineage.iter().any(
            |link| link.target == a.reference() && link.relation == LineageRelation::MergedFrom
        )
    );
    assert_eq!(
        app.concept_by_alias(&actor, &subject.id, " ORIGINAL CONCEPT ")
            .await
            .unwrap()
            .id,
        b.id
    );
    let collision = SplitConceptRequest {
        parent: expected(&survivor),
        children: vec![content("temporary child"), content("survivor concept")],
        evidence: evidence.clone(),
        reason: "rollback probe".into(),
    };
    assert_eq!(
        app.split_concept(&actor, "collision", collision)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        app.concept_by_alias(&actor, &subject.id, "temporary child")
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.get_structured(&actor, &b.id, None)
            .await
            .unwrap()
            .revision,
        2
    );
    let split = app
        .split_concept(
            &actor,
            "split",
            SplitConceptRequest {
                parent: expected(&survivor),
                children: vec![
                    content("specific concept one"),
                    content("specific concept two"),
                ],
                evidence: evidence.clone(),
                reason: "two distinct meanings".into(),
            },
        )
        .await
        .unwrap();
    let children: Vec<StructuredObject> =
        serde_json::from_value(split.result["children"].clone()).unwrap();
    assert_eq!(children.len(), 2);
    assert!(children.iter().all(|child| {
        concept(child).unwrap().lineage.iter().any(|link| {
            link.target == survivor.reference() && link.relation == LineageRelation::SplitFrom
        })
    }));
    assert_eq!(
        app.get_structured(&actor, &survivor.id, None)
            .await
            .unwrap()
            .revision,
        2
    );
    let members = vec![
        EpisodeMember {
            source: EvidenceSource::Observation {
                observation_id: first.id,
            },
            role: "experience".into(),
            ordinal: 0,
        },
        EpisodeMember {
            source: EvidenceSource::Observation {
                observation_id: second.id,
            },
            role: "experience".into(),
            ordinal: 1,
        },
    ];
    let original = EpisodeContent {
        title: "whole session".into(),
        track: "work".into(),
        experience: TimeRange {
            domain: "fixture".into(),
            start: Some(0),
            end: Some(30),
        },
        parent: None,
        members: members.clone(),
        boundary_explanation: "session source".into(),
        lineage: vec![],
    };
    let source = create(
        &app,
        &namespace.id,
        &subject.id,
        "episode",
        CognitiveContent::Episode(original.clone()),
        &evidence,
    )
    .await;
    let segments = members
        .into_iter()
        .enumerate()
        .map(|(index, mut member)| {
            member.ordinal = 0;
            EpisodeContent {
                title: format!("part {index}"),
                experience: TimeRange {
                    domain: "fixture".into(),
                    start: Some((index * 15) as i64),
                    end: Some(((index + 1) * 15) as i64),
                },
                members: vec![member],
                boundary_explanation: "revised local boundary".into(),
                ..original.clone()
            }
        })
        .collect();
    let request = ResegmentEpisodesRequest {
        sources: vec![expected(&source)],
        segments,
        evidence: evidence.clone(),
        reason: "observed boundary change".into(),
    };
    let partition = app
        .resegment_episodes(&actor, "resegment", request.clone())
        .await
        .unwrap();
    assert_eq!(
        partition,
        app.resegment_episodes(&actor, "resegment", request)
            .await
            .unwrap()
    );
    let episodes: Vec<StructuredObject> =
        serde_json::from_value(partition.result["episodes"].clone()).unwrap();
    assert_eq!(episodes.len(), 2);
    assert!(episodes.iter().all(|child| {
        episode(child).unwrap().lineage.iter().any(|link| {
            link.target == source.reference() && link.relation == LineageRelation::ResegmentedFrom
        })
    }));
    assert!(
        app.structured_history(&actor, &source.id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .deleted
    );
    assert_eq!(
        app.get_structured(&actor, &source.id, Some(1))
            .await
            .unwrap()
            .revision,
        1
    );
    app.close().await;
}
fn concept(object: &StructuredObject) -> Result<&ConceptContent> {
    if let CognitiveContent::Concept(value) = &object.body {
        Ok(value)
    } else {
        Err(Error::invalid("expected Concept"))
    }
}
fn episode(object: &StructuredObject) -> Result<&EpisodeContent> {
    if let CognitiveContent::Episode(value) = &object.body {
        Ok(value)
    } else {
        Err(Error::invalid("expected Episode"))
    }
}
