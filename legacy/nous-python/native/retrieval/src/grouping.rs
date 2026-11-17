//! Shared score-ordered group collection and fair union allocation.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct LaneCoverage {
    pub examined_rows: usize,
    pub eligible_groups_known: Option<usize>,
    pub recall_exhausted: bool,
    pub truncated: bool,
    pub reason: Option<String>,
}
/// Input is ordered by the lane's real score, with a stable tie key.
pub fn collect(
    ordered: impl IntoIterator<Item = (u64, f64)>,
    groups: &[u64],
    limit: usize,
    passages: usize,
) -> Vec<(u64, f64)> {
    let mut selected = BTreeMap::<u64, usize>::new();
    let mut hits = Vec::new();
    for (row, score) in ordered {
        let group = groups[row as usize];
        if !selected.contains_key(&group) && selected.len() >= limit {
            continue;
        }
        let count = selected.entry(group).or_default();
        if *count < passages {
            *count += 1;
            hits.push((row, score));
        }
    }
    hits
}
/// Reserve a representative for every selected group before filling passage slots.
pub fn fair_rows(ordered: &[u64], groups: &[u64], limit: usize, passages: usize) -> BTreeSet<u64> {
    let mut counts = BTreeMap::<u64, usize>::new();
    let mut selected = BTreeSet::new();
    for row in ordered {
        if selected.len() == limit {
            break;
        }
        if let std::collections::btree_map::Entry::Vacant(entry) =
            counts.entry(groups[*row as usize])
        {
            entry.insert(1);
            selected.insert(*row);
        }
    }
    for row in ordered {
        if selected.len() == limit {
            break;
        }
        if selected.contains(row) {
            continue;
        }
        if let Some(count) = counts.get_mut(&groups[*row as usize]) {
            if *count < passages {
                *count += 1;
                selected.insert(*row);
            }
        }
    }
    selected
}
