// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_retrieval::{affected_families, family_config_keys};

#[test]
fn wave_build_keys_invalidate_topology_but_readout_does_not() {
    assert_eq!(
        affected_families(&["topology.wave.max_neighbors_per_node"]),
        ["topology"]
    );
    assert!(affected_families(&["topology.wave.max_hops"]).is_empty());
    assert!(affected_families(&["serving.topology.enabled"]).is_empty());
}

#[test]
fn synopsis_and_epa_map_only_to_consuming_families() {
    assert_eq!(affected_families(&["retrieval.epa"]), ["dense"]);
    assert_eq!(
        affected_families(&["episode.synopsis"]),
        ["lexical", "dense", "topology"]
    );
    assert!(family_config_keys("concept").unwrap().is_empty());
}

#[test]
fn canonical_paths_are_available_only_for_physical_families() {
    assert!(
        family_config_keys("topology")
            .unwrap()
            .contains(&"retrieval.vcp.assets")
    );
    assert!(family_config_keys("query").is_none());
}
