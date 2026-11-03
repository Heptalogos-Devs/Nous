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
