//! Adapted from VCPToolBox rust-vexus-lite/src/rivermemo_topology_v3.rs
//! Upstream commit d1b40c863273cbbba082fb7657671424768c4a53.
//! CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
//! Host/database access is replaced by immutable typed inputs.

use std::collections::{HashMap, HashSet};
fn positive(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}
fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
#[derive(Debug, Clone, Default)]
pub struct NativeArtifact {
    pub node_ids: Vec<i64>,
    pub node_index: HashMap<i64, usize>,
    pub row_offsets: Vec<usize>,
    pub targets: Vec<usize>,
    pub weights: Vec<f64>,
    pub inbound: HashMap<i64, f64>,
    pub max_inbound: f64,
    pub anchor_gain: HashMap<i64, f64>,
    pub wormhole_edges: HashSet<(i64, i64)>,
    pub provenance: HashMap<(i64, i64), Vec<(i64, f64)>>,
    pub node_anchor_gain: Vec<f64>,
    pub edge_is_wormhole: Vec<bool>,
}

impl NativeArtifact {
    fn validate_layout(&self) -> Result<(), String> {
        let n = self.node_ids.len();
        let e = self.targets.len();
        if self.node_index.len() != n
            || self
                .node_ids
                .iter()
                .enumerate()
                .any(|(index, id)| *id <= 0 || self.node_index.get(id) != Some(&index))
            || self.row_offsets.len() != n + 1
            || self.row_offsets.first() != Some(&0)
            || self.row_offsets.last() != Some(&e)
            || self.row_offsets.windows(2).any(|row| row[0] > row[1])
            || self.weights.len() != e
            || self.targets.iter().any(|target| *target >= n)
            || self
                .weights
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return Err(
                "MEMO_CSR_INVALID: IDs, offsets, targets or weights are inconsistent".into(),
            );
        }
        if !self.max_inbound.is_finite()
            || self.max_inbound < 0.0
            || self
                .inbound
                .iter()
                .chain(self.anchor_gain.iter())
                .any(|(id, value)| *id <= 0 || !value.is_finite() || *value < 0.0)
            || self
                .wormhole_edges
                .iter()
                .any(|(a, b)| !self.node_index.contains_key(a) || !self.node_index.contains_key(b))
            || self.provenance.iter().any(|((a, b), values)| {
                *a <= 0
                    || *b <= 0
                    || values
                        .iter()
                        .any(|(id, mass)| *id <= 0 || !mass.is_finite() || *mass < 0.0)
            })
        {
            return Err("MEMO_GRAPH_METADATA_INVALID: non-finite values or invalid IDs".into());
        }
        Ok(())
    }

    /// Validate external CSR input before deriving the sorted query arrays.
    pub fn try_prepared(self) -> Result<Self, String> {
        self.validate_layout()?;
        Ok(self.prepared())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_layout()?;
        if self.node_anchor_gain.len() != self.node_ids.len()
            || self.edge_is_wormhole.len() != self.targets.len()
            || self
                .node_anchor_gain
                .iter()
                .enumerate()
                .any(|(index, gain)| {
                    !gain.is_finite()
                        || *gain
                            != self
                                .anchor_gain
                                .get(&self.node_ids[index])
                                .copied()
                                .unwrap_or(1.0)
                })
        {
            return Err("MEMO_GRAPH_UNPREPARED: derive query arrays with try_prepared".into());
        }
        for source in 0..self.node_ids.len() {
            for cursor in self.row_offsets[source]..self.row_offsets[source + 1] {
                if self.edge_is_wormhole[cursor]
                    != self
                        .wormhole_edges
                        .contains(&(self.node_ids[source], self.node_ids[self.targets[cursor]]))
                {
                    return Err("MEMO_GRAPH_UNPREPARED: wormhole flags are inconsistent".into());
                }
            }
        }
        Ok(())
    }

    /// Derive immutable hot-path arrays after CSR validation, for both freshly
    /// built and restored assets. Temporary workspace is bounded to one row.
    pub(crate) fn prepared(mut self) -> Self {
        self.node_anchor_gain = self
            .node_ids
            .iter()
            .map(|id| self.anchor_gain.get(id).copied().unwrap_or(1.0))
            .collect();
        self.edge_is_wormhole = vec![false; self.targets.len()];
        for source in 0..self.node_ids.len() {
            let start = self.row_offsets[source];
            let end = self.row_offsets[source + 1];
            let mut row: Vec<(usize, f64)> = (start..end)
                .map(|cursor| (self.targets[cursor], self.weights[cursor]))
                .collect();
            row.sort_by(|left, right| {
                positive(right.1)
                    .total_cmp(&positive(left.1))
                    .then_with(|| self.node_ids[left.0].cmp(&self.node_ids[right.0]))
            });
            for (cursor, (target, weight)) in (start..end).zip(row) {
                self.targets[cursor] = target;
                self.weights[cursor] = weight;
                self.edge_is_wormhole[cursor] = self
                    .wormhole_edges
                    .contains(&(self.node_ids[source], self.node_ids[target]));
            }
        }
        self
    }

    pub fn edge_weight(&self, source_id: i64, target_id: i64) -> f64 {
        let Some(&source) = self.node_index.get(&source_id) else {
            return 0.0;
        };
        let Some(&target) = self.node_index.get(&target_id) else {
            return 0.0;
        };
        let start = self.row_offsets[source];
        let end = self.row_offsets[source + 1];
        for cursor in start..end {
            if self.targets[cursor] == target {
                return positive(self.weights[cursor]);
            }
        }
        0.0
    }

    pub fn independent_fraction(&self, source: i64, target: i64, file_id: i64) -> f64 {
        self.independent_fraction_excluding(source, target, &[file_id])
    }

    /// Derived candidates can depend on several originals. Exclude all their
    /// evidence roots while retaining the pinned kernel's weighting and floor.
    pub fn independent_fraction_excluding(&self, source: i64, target: i64, roots: &[i64]) -> f64 {
        let Some(contributions) = self.provenance.get(&(source, target)) else {
            return 1.0;
        };
        let total: f64 = contributions.iter().map(|item| item.1).sum();
        if total <= 0.0 {
            return 1.0;
        }
        let own: f64 = contributions
            .iter()
            .filter(|item| roots.contains(&item.0))
            .map(|item| item.1)
            .sum();
        clamp01(1.0 - own / total).max(0.15)
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
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
}
