use nous_vcp::graph::NativeArtifact;
use nous_vcp::projection::{Basin, grow, seed_basins};
use nous_vcp::sensing::{SenseConfig, SenseInput, SenseSeed, sense_typed};
use std::collections::HashSet;

fn graph(count: usize, edges: &[(usize, usize, f64)]) -> NativeArtifact {
    let mut g = NativeArtifact {
        node_ids: (1..=count as i64).collect(),
        node_index: (1..=count as i64)
            .enumerate()
            .map(|(i, id)| (id, i))
            .collect(),
        ..NativeArtifact::default()
    };
    g.row_offsets.push(0);
    for source in 0..count {
        for &(s, t, w) in edges {
            if s == source {
                g.targets.push(t);
                g.weights.push(w);
            }
        }
        g.row_offsets.push(g.targets.len());
    }
    g.try_prepared().unwrap()
}
fn basin(id: i64, importance: f64) -> Basin {
    Basin {
        id,
        importance,
        inner: vec![id],
    }
}
fn assert_paths(g: &NativeArtifact, support: &[i64], core: &[i64]) {
    for (position, id) in support.iter().enumerate() {
        if core.contains(id) {
            continue;
        }
        assert!(
            support[..position]
                .iter()
                .any(|p| g.edge_weight(*p, *id) > 0.0),
            "node {} lacks an admitted predecessor",
            id
        );
    }
}

#[test]
fn shared_256_cap_and_weak_basin_reserved_exploration() {
    let mut edges = Vec::new();
    for i in 2..602 {
        edges.push((if i % 2 == 0 { 0 } else { 1 }, i, 0.001));
    }
    let g = graph(602, &edges);
    let mut local = vec![0.001; 602];
    let mut transfer = local.clone();
    for i in 2..602 {
        if i % 2 == 0 {
            local[i] = 0.1;
        } else {
            transfer[i] = 0.1;
        }
    }
    let domain = g.node_ids.clone();
    let basins = [basin(1, 100.0), basin(2, 0.0001)];
    let result = grow(
        &g,
        [&local, &transfer],
        [&domain, &domain],
        &[1, 2],
        &basins,
        256,
    )
    .unwrap();
    assert_eq!(result.diagnostics.union_nodes, 256);
    assert!(
        result
            .diagnostics
            .lanes
            .iter()
            .all(|l| serde_json::to_value(l).unwrap()["reservedNodes"] == 8)
    );
    assert!(result.diagnostics.lanes.iter().any(|l| {
        serde_json::to_value(l).unwrap()["outerNodes"]
            .as_u64()
            .unwrap()
            > 8
    }));
    for support in &result.ids {
        assert_paths(&g, support, &[1, 2]);
    }
    let again = grow(
        &g,
        [&local, &transfer],
        [&domain, &domain],
        &[2, 1],
        &basins,
        256,
    )
    .unwrap();
    assert_eq!(result.ids, again.ids);
    assert_eq!(
        serde_json::to_value(result.diagnostics).unwrap(),
        serde_json::to_value(again.diagnostics).unwrap()
    );
}

#[test]
fn overlap_deduplicates_without_inflating_field_mass() {
    let g = graph(5, &[(0, 2, 0.5), (1, 2, 0.5), (2, 3, 0.5)]);
    let field = [0.1, 0.1, 0.1, 0.1, 0.6]; // Unreachable node keeps the target open.
    let domain = g.node_ids.clone();
    let result = grow(
        &g,
        [&field, &field],
        [&domain, &domain],
        &[1, 2],
        &[basin(1, 1.0), basin(2, 1.0)],
        4,
    )
    .unwrap();
    assert_eq!(result.diagnostics.union_nodes, 4);
    assert_eq!(result.diagnostics.overlap_nodes, 2);
    for ids in &result.ids {
        assert_eq!(ids.len(), 4);
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), 4);
        assert!(!ids.contains(&5));
        assert_paths(&g, ids, &[1, 2]);
    }
}

#[test]
fn independent_field_target_does_not_borrow_other_field_nodes() {
    let g = graph(3, &[(0, 1, 0.5), (1, 2, 0.5)]);
    let local = [0.9, 0.05, 0.05];
    let transfer = [0.1, 0.2, 0.7];
    let result = grow(
        &g,
        [&local, &transfer],
        [&[1], &[3, 2]],
        &[1],
        &[basin(1, 1.0)],
        3,
    )
    .unwrap();
    assert_eq!(result.ids[0], vec![1]);
    assert_eq!(result.ids[1], vec![1, 2, 3]);
    assert_eq!(result.diagnostics.union_nodes, 3);
}

#[test]
fn guaranteed_exploration_crosses_low_mass_bridge_in_distance_order() {
    let g = graph(5, &[(0, 1, 0.5), (1, 2, 0.5), (0, 3, 0.01)]);
    let field = [0.1, 0.001, 0.5, 0.1, 0.299];
    let domain = g.node_ids.clone();
    let result = grow(
        &g,
        [&field, &field],
        [&domain, &domain],
        &[1],
        &[basin(1, 1.0)],
        3,
    )
    .unwrap();
    for ids in &result.ids {
        assert_eq!(ids, &[1, 2, 3]);
        assert_paths(&g, ids, &[1]);
    }
    assert!(
        result
            .diagnostics
            .lanes
            .iter()
            .all(|l| serde_json::to_value(l).unwrap()["reservedNodes"] == 2)
    );
}

#[test]
fn exhausted_basin_returns_budget_to_other_lanes() {
    let g = graph(6, &[(1, 2, 0.5), (2, 3, 0.5), (3, 4, 0.5), (4, 5, 0.5)]);
    let field = [1.0; 6];
    let domain = g.node_ids.clone();
    let result = grow(
        &g,
        [&field, &field],
        [&domain, &domain],
        &[1, 2],
        &[basin(1, 100.0), basin(2, 0.001)],
        6,
    )
    .unwrap();
    assert_eq!(result.diagnostics.union_nodes, 6);
    assert!(
        result
            .diagnostics
            .lanes
            .iter()
            .filter(|l| serde_json::to_value(l).unwrap()["basinId"] == 1)
            .all(|l| serde_json::to_value(l).unwrap()["outerNodes"] == 0)
    );
}

#[test]
fn inner_traversal_does_not_consume_reserved_outer_slots() {
    let edges: Vec<_> = (0..29).map(|i| (i, i + 1, 0.5)).collect();
    let g = graph(30, &edges);
    let field = [1.0; 30];
    let domain = g.node_ids.clone();
    let core: Vec<_> = (1..=20).collect();
    let b = Basin {
        id: 1,
        inner: core.clone(),
        importance: 1.0,
    };
    let result = grow(&g, [&field, &field], [&domain, &domain], &core, &[b], 30).unwrap();
    assert!(
        result
            .diagnostics
            .lanes
            .iter()
            .all(|l| serde_json::to_value(l).unwrap()["reservedNodes"] == 8)
    );
    assert_eq!(result.diagnostics.union_nodes, 30);
}

#[test]
fn work_is_bounded_even_without_node_cap() {
    let edges: Vec<_> = (1..70000).map(|i| (0, i, 0.00001)).collect();
    let g = graph(70000, &edges);
    let field = vec![1.0; 70000];
    let domain = g.node_ids.clone();
    let result = grow(
        &g,
        [&field, &field],
        [&domain, &domain],
        &[1],
        &[basin(1, 1.0)],
        0,
    )
    .unwrap();
    assert!(result.diagnostics.scanned_edges <= 65_536);
    assert!(result.diagnostics.settled_states <= 8192);
    assert_eq!(result.diagnostics.stop_reason, "work_limit");
}

#[test]
fn observation_forest_preserves_all_nodes_despite_misleading_lineage() {
    let g = graph(5, &[(0, 2, 0.5), (1, 2, 0.5), (2, 3, 0.5)]);
    let mut observation = sense_typed(
        &g,
        "test",
        SenseInput {
            query_id: None,
            seeds: vec![
                SenseSeed {
                    id: 1,
                    energy: 1.0,
                    source_type: "seed".into(),
                },
                SenseSeed {
                    id: 2,
                    energy: 1.0,
                    source_type: "seed".into(),
                },
                SenseSeed {
                    id: 5,
                    energy: 1.0,
                    source_type: "seed".into(),
                },
            ],
            config: SenseConfig {
                max_safe_hops: 3,
                minimum_injected_current: 0.001,
                firing_threshold: 0.001,
                ..SenseConfig::default()
            },
        },
    )
    .unwrap();
    // Reassign descendant labels to a root with no path. Never trust this as a partition.
    for node in &mut observation.nodes {
        if node.hop > 0 {
            node.seed_id = Some(5);
        }
    }
    let basins = seed_basins(&observation, &g);
    let assigned: Vec<_> = basins
        .iter()
        .flat_map(|b| b.inner.iter().copied())
        .collect();
    assert_eq!(assigned.len(), observation.nodes.len());
    assert_eq!(
        assigned.iter().copied().collect::<HashSet<_>>().len(),
        assigned.len()
    );
    assert_eq!(basins.iter().find(|b| b.id == 5).unwrap().inner, vec![5]);
    assert_eq!(
        basins.iter().find(|b| b.id == 1).unwrap().inner,
        vec![1, 3, 4]
    );
    observation.edges.clear();
    let isolated = seed_basins(&observation, &g);
    assert!(isolated.iter().all(|b| b.inner.len() == 1));
}
