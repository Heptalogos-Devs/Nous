// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn witnesses_only_reachable_bounded_paths_and_terminates_on_cycles() {
    let paths = activated_routes([1, 5], [(1, 2), (2, 1), (2, 3), (3, 4), (5, 4), (8, 9)], 2);
    assert_eq!(paths[&3], [1, 2, 3]);
    assert_eq!(paths[&4], [5, 4]);
    assert!(!paths.contains_key(&8));
    assert!(!activated_routes([1], [(1, 2), (2, 3)], 1).contains_key(&3));
}
