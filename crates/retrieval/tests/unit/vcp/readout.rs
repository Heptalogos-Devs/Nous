// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn document_source_overlap_conserves_mass_and_unknown_lineage_is_not_independent() {
    let own = BTreeSet::from(["external:shared".into()]);
    let same = BTreeSet::from(["external:shared".into()]);
    let partial = BTreeSet::from(["external:shared".into(), "external:independent".into()]);
    let unknown = BTreeSet::from(["unknown-dependency:memory_revision:x".into()]);
    let empty = BTreeSet::new();
    let docs = std::collections::BTreeMap::from([
        ("memory_revision:same".into(), &same),
        ("memory_revision:partial".into(), &partial),
        ("memory_revision:unknown".into(), &unknown),
        ("memory_revision:empty".into(), &empty),
    ]);
    for (root, expected) in [
        ("memory_revision:same", (0.8, 0.0)),
        ("memory_revision:partial", (0.4, 0.4)),
        ("memory_revision:unknown", (0.8, 0.0)),
        ("memory_revision:empty", (0.8, 0.0)),
        ("external:independent", (0.0, 0.8)),
        ("external:shared", (0.8, 0.0)),
    ] {
        let actual = root_mass_ownership(root, 0.8, &own, &docs);
        assert_eq!(actual, expected);
        assert!((actual.0 + actual.1 - 0.8).abs() < 1e-12);
    }
}
