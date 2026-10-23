use nous_vcp::graph::NativeArtifact;
#[test]
fn derivations_exclude_all_own_roots_without_counting_them_twice() {
    let mut graph = NativeArtifact::default();
    graph
        .provenance
        .insert((1, 2), vec![(10, 2.0), (20, 3.0), (30, 5.0)]);
    assert_eq!(graph.independent_fraction(1, 2, 10), 0.8);
    assert_eq!(graph.independent_fraction_excluding(1, 2, &[10, 20]), 0.5);
    assert_eq!(
        graph.independent_fraction_excluding(1, 2, &[10, 20, 10]),
        0.5
    );
    assert_eq!(
        graph.independent_fraction_excluding(1, 2, &[10, 20, 30]),
        0.15
    );
}
