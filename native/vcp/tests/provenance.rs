use nous_vcp::graph::NativeArtifact;

#[test]
fn derived_candidate_excludes_every_own_root_from_independent_support() {
    let graph = NativeArtifact {
        provenance: [((1, 2), vec![(101, 3.0), (202, 1.0), (303, 2.0)])]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    // A summary derived from A and B may count only C's two units of support.
    assert!((graph.independent_fraction_excluding(1, 2, &[101, 202]) - 2.0 / 6.0).abs() < 1e-15);
    assert!(
        (graph.independent_fraction_excluding(1, 2, &[101, 101, 202]) - 2.0 / 6.0).abs() < 1e-15
    );
    assert_eq!(
        graph.independent_fraction_excluding(1, 2, &[101, 202, 303]),
        0.15
    );
    assert_eq!(graph.independent_fraction(1, 2, 101), 0.5);
}

#[test]
fn joint_support_is_excluded_as_one_group_and_floor_is_not_evidence() {
    use nous_vcp::graph::SupportGroup;
    let graph = NativeArtifact {
        provenance: [((1, 2), vec![(10, 2.0), (11, 3.0)])].into_iter().collect(),
        support_groups: [
            (
                10,
                SupportGroup {
                    required_roots: vec![101, 202],
                    kind: "interpretation".into(),
                },
            ),
            (
                11,
                SupportGroup {
                    required_roots: vec![303],
                    kind: "source".into(),
                },
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    };
    assert_eq!(graph.independent_support_fraction(1, 2, &[101]), Some(0.6));
    assert_eq!(
        graph.independent_support_fraction(1, 2, &[101, 303]),
        Some(0.0)
    );
    assert_eq!(
        graph.independent_fraction_excluding(1, 2, &[101, 303]),
        0.15
    );
    assert_eq!(graph.independent_support_fraction(2, 3, &[]), None);
}

#[test]
fn long_occurrence_sequences_contribute_to_geometry() {
    use nous_vcp::{
        artifact::FileTag,
        geometry::{GeometryInput, GeometryTag, build_geometry},
    };
    let tags = (1..=5)
        .map(|id| GeometryTag {
            id,
            name: format!("concept-{id}"),
            vector_version: 1,
            vector: vec![1.0, id as f32 * 0.1, 0.3],
        })
        .collect();
    let memberships = (0..101)
        .map(|i| FileTag {
            file_id: 1,
            tag_id: i % 5 + 1,
            position: i + 1,
        })
        .collect();
    let geometry = build_geometry(
        GeometryInput {
            dimension: 3,
            model_sig: "test".into(),
            config: serde_json::json!({}),
            tags,
            memberships,
            edges: None,
            recorded_at_ms: 0,
        },
        None,
    )
    .unwrap();
    assert_eq!(geometry.computed_count, 5);
    assert!(geometry.intrinsic.iter().all(|value| value.count == 4));
}
