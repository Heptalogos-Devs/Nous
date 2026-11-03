//! Consumes immutable typed inputs.
//! Request-local seed-basin projection. Never modifies Sense, scalar fields or Omega.
use crate::graph::NativeArtifact;
use crate::sensing::SenseOutput;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet, VecDeque};

const RESERVED: usize = 8;
const STATE_LIMIT: usize = 8192;
const EDGE_LIMIT: usize = 65536;
const BATCH: usize = 4;

fn positive(x: f64) -> f64 {
    if x.is_finite() { x.max(0.0) } else { 0.0 }
}

#[derive(Clone, Debug)]
pub struct Basin {
    pub id: i64,
    pub inner: Vec<i64>,
    pub importance: f64,
}

/// A deterministic, disjoint observation-edge forest rooted at the recorded seeds.
/// seed_id is representative provenance, not exact multi-source lineage. Forest
/// assignment validates paths instead of trusting a potentially disconnected label group.
pub fn seed_basins(observation: &SenseOutput, graph: &NativeArtifact) -> Vec<Basin> {
    let nodes: BTreeMap<_, _> = observation
        .nodes
        .iter()
        .filter(|n| graph.node_index.contains_key(&n.id))
        .map(|n| (n.id, n))
        .collect();
    let mut roots: Vec<i64> = nodes
        .values()
        .filter(|n| n.hop == 0)
        .map(|n| n.id)
        .chain(
            nodes
                .values()
                .filter_map(|n| n.seed_id)
                .filter(|id| nodes.contains_key(id)),
        )
        .collect();
    roots.sort_unstable();
    roots.dedup();
    let mut edges = HashMap::<i64, Vec<i64>>::new();
    for edge in &observation.edges {
        if positive(edge.flow) > 0.0
            && nodes.contains_key(&edge.source_id)
            && nodes.contains_key(&edge.target_id)
            && graph.edge_weight(edge.source_id, edge.target_id) > 0.0
        {
            edges
                .entry(edge.source_id)
                .or_default()
                .push(edge.target_id);
        }
    }
    for row in edges.values_mut() {
        row.sort_unstable();
        row.dedup();
    }
    let mut owner = HashMap::new();
    let mut basins = Vec::new();
    let mut queue = VecDeque::new();
    for root in roots {
        owner.insert(root, basins.len());
        queue.push_back(root);
        basins.push(Basin {
            id: root,
            inner: vec![root],
            importance: 0.0,
        });
    }
    // Multiple roots are reserved before traversal, so a strong source cannot
    // consume another seed's root. Each assigned node has a same-basin predecessor.
    while let Some(source) = queue.pop_front() {
        let basin = owner[&source];
        if let Some(row) = edges.get(&source) {
            for &target in row {
                if let std::collections::hash_map::Entry::Vacant(entry) = owner.entry(target) {
                    entry.insert(basin);
                    basins[basin].inner.push(target);
                    queue.push_back(target);
                }
            }
        }
    }
    // Truncated/unknown provenance must not silently drop core nodes or invent paths.
    for &id in nodes.keys() {
        if let std::collections::hash_map::Entry::Vacant(entry) = owner.entry(id) {
            entry.insert(basins.len());
            basins.push(Basin {
                id,
                inner: vec![id],
                importance: 0.0,
            });
        }
    }
    for basin in &mut basins {
        basin.inner.sort_unstable();
        basin.importance = basin
            .inner
            .iter()
            .map(|id| positive(nodes[id].energy))
            .sum();
    }
    basins.sort_by_key(|b| b.id);
    basins
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaneDiagnostics {
    basin_id: i64,
    field: usize,
    inner_nodes: usize,
    outer_nodes: usize,
    reserved_nodes: usize,
    new_union_nodes: usize,
    settled_nodes: usize,
    scanned_edges: usize,
    radius: f64,
    truncated: bool,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthDiagnostics {
    pub budget: usize,
    pub core_nodes: usize,
    pub union_nodes: usize,
    pub overlap_nodes: usize,
    pub settled_states: usize,
    pub scanned_edges: usize,
    pub stop_reason: String,
    pub lanes: Vec<LaneDiagnostics>,
}
pub struct Supports {
    pub ids: [Vec<i64>; 2],
    pub diagnostics: GrowthDiagnostics,
}

#[derive(Clone, Copy)]
struct Arrival {
    distance: f64,
    id: i64,
    index: usize,
}
impl PartialEq for Arrival {
    fn eq(&self, rhs: &Self) -> bool {
        self.distance.total_cmp(&rhs.distance) == Ordering::Equal && self.id == rhs.id
    }
}
impl Eq for Arrival {}
impl PartialOrd for Arrival {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        Some(self.cmp(rhs))
    }
}
impl Ord for Arrival {
    fn cmp(&self, rhs: &Self) -> Ordering {
        rhs.distance
            .total_cmp(&self.distance)
            .then_with(|| rhs.id.cmp(&self.id))
    }
}
struct Lane {
    field: usize,
    importance: f64,
    frontier: BinaryHeap<Arrival>,
    distance: HashMap<usize, f64>,
    settled: HashSet<usize>,
    inner: HashSet<i64>,
    gain: f64,
    diagnostics: LaneDiagnostics,
}
impl Lane {
    fn new(field: usize, basin: &Basin, graph: &NativeArtifact) -> Self {
        let inner: HashSet<_> = basin.inner.iter().copied().collect();
        let distance: HashMap<_, _> = basin
            .inner
            .iter()
            .map(|id| (graph.node_index[id], 0.0))
            .collect();
        let frontier = basin
            .inner
            .iter()
            .map(|&id| Arrival {
                distance: 0.0,
                id,
                index: graph.node_index[&id],
            })
            .collect();
        Self {
            field,
            importance: positive(basin.importance),
            frontier,
            distance,
            settled: HashSet::new(),
            inner,
            gain: 0.0,
            diagnostics: LaneDiagnostics {
                basin_id: basin.id,
                field,
                inner_nodes: basin.inner.len(),
                ..LaneDiagnostics::default()
            },
        }
    }
    fn next(&mut self) -> Option<Arrival> {
        while let Some(a) = self.frontier.pop() {
            if !self.settled.contains(&a.index)
                && self.distance.get(&a.index).copied() == Some(a.distance)
            {
                return Some(a);
            }
        }
        None
    }
}

/// One shared union cap, independently admitted field supports, finite work.
/// Distances are from each connected inner boundary, NOT from a single centroid.
pub fn grow(
    graph: &NativeArtifact,
    fields: [&[f64]; 2],
    domains: [&[i64]; 2],
    core: &[i64],
    basins: &[Basin],
    limit: usize,
) -> Result<Supports, String> {
    let mut core: Vec<i64> = core
        .iter()
        .copied()
        .filter(|id| graph.node_index.contains_key(id))
        .collect();
    core.sort_unstable();
    core.dedup();
    if limit > 0 && core.len() > limit {
        return Err(format!(
            "MEMO_PROJECTION_CORE_EXCEEDS_BUDGET: core has {} nodes, shared budget is {}; increase maxFieldProjectionNodes or use 0",
            core.len(),
            limit
        ));
    }
    let mut selected: [HashSet<i64>; 2] = std::array::from_fn(|_| core.iter().copied().collect());
    let mut ids = [core.clone(), core.clone()];
    let mut union: HashSet<i64> = core.iter().copied().collect();
    let totals: [f64; 2] = fields.map(|f| f.iter().copied().map(positive).sum());
    let targets: [f64; 2] = std::array::from_fn(|f| {
        domains[f]
            .iter()
            .filter_map(|id| graph.node_index.get(id))
            .map(|&i| positive(fields[f][i]))
            .sum()
    });
    let mut mass: [f64; 2] = std::array::from_fn(|f| {
        core.iter()
            .map(|id| positive(fields[f][graph.node_index[id]]))
            .sum()
    });
    let cutoffs = fields.map(|f| f.iter().copied().map(positive).fold(0.0_f64, f64::max) * 1e-12);
    // Test/legacy callers without an observation use explicit singleton basins.
    let fallback;
    let basins = if basins.is_empty() {
        fallback = core
            .iter()
            .map(|&id| Basin {
                id,
                inner: vec![id],
                importance: 1.0,
            })
            .collect::<Vec<_>>();
        &fallback[..]
    } else {
        basins
    };
    let mut lanes = Vec::new();
    for basin in basins {
        if basin.inner.is_empty() || !basin.inner.iter().all(|id| union.contains(id)) {
            return Err("MEMO_PROJECTION_INVALID_BASIN: inner nodes must belong to core".into());
        }
        for f in 0..2 {
            lanes.push(Lane::new(f, basin, graph));
        }
    }
    let mut coverage = HashMap::<i64, HashSet<i64>>::new();
    let mut states = 0usize;
    let mut scanned = 0usize;
    let mut discovered = lanes.iter().map(|l| l.distance.len()).sum::<usize>();
    let mut reason = "frontier_exhausted";
    let mut cursor = 0usize;
    loop {
        if mass.iter().zip(targets).all(|(&m, t)| m >= t) {
            reason = "mass_target";
            break;
        }
        if states >= STATE_LIMIT || scanned >= EDGE_LIMIT {
            reason = "work_limit";
            break;
        }
        let active = |l: &Lane| mass[l.field] < targets[l.field] && !l.frontier.is_empty();
        // Guaranteed exploration is round robin and ignores gain entirely.
        let reserved = (0..lanes.len())
            .map(|offset| (cursor + offset) % lanes.len())
            .find(|&i| active(&lanes[i]) && lanes[i].diagnostics.outer_nodes < RESERVED);
        let choice = reserved.or_else(|| {
            lanes
                .iter()
                .enumerate()
                .filter(|(_, l)| active(l))
                .max_by(|(ia, a), (ib, b)| {
                    let priority = |l: &Lane| {
                        l.importance.sqrt() * l.gain.max(1e-12)
                            / (1.0 + l.diagnostics.outer_nodes as f64 / RESERVED as f64)
                    };
                    priority(a).total_cmp(&priority(b)).then_with(|| ib.cmp(ia))
                })
                .map(|(i, _)| i)
        });
        let Some(index) = choice else { break };
        cursor = (index + 1) % lanes.len();
        let lane = &mut lanes[index];
        let mut batch_gain = 0.0;
        let mut outer = 0usize;
        // Inner traversal never spends an outer reserved opportunity.
        while outer < if reserved.is_some() { 1 } else { BATCH }
            && states < STATE_LIMIT
            && scanned < EDGE_LIMIT
        {
            let Some(a) = lane.next() else { break };
            states += 1;
            lane.diagnostics.settled_nodes += 1;
            lane.settled.insert(a.index);
            if !union.contains(&a.id) && limit > 0 && union.len() >= limit {
                reason = "budget";
                continue; // Never expose successors through an unadmitted node.
            }
            let f = lane.field;
            if union.insert(a.id) {
                lane.diagnostics.new_union_nodes += 1;
            }
            let is_outer = !lane.inner.contains(&a.id);
            if is_outer {
                outer += 1;
                lane.diagnostics.outer_nodes += 1;
                if reserved.is_some() {
                    lane.diagnostics.reserved_nodes += 1;
                }
            }
            if selected[f].insert(a.id) {
                ids[f].push(a.id);
                let value = positive(fields[f][a.index]);
                mass[f] += value;
                batch_gain += value / totals[f].max(1e-30);
            }
            coverage
                .entry(a.id)
                .or_default()
                .insert(lane.diagnostics.basin_id);
            lane.diagnostics.radius = a.distance;
            for edge in graph.row_offsets[a.index]..graph.row_offsets[a.index + 1] {
                if scanned >= EDGE_LIMIT {
                    break;
                }
                scanned += 1;
                lane.diagnostics.scanned_edges += 1;
                let weight = graph.weights[edge];
                if !weight.is_finite() || weight <= 0.0 || weight > 1.0 {
                    continue;
                }
                let target = graph.targets[edge];
                let value = positive(fields[f][target]);
                if value <= 0.0 || value < cutoffs[f] || lane.settled.contains(&target) {
                    continue;
                }
                let distance = a.distance - weight.ln();
                let previous = lane.distance.get(&target).copied();
                if previous.is_none() && discovered >= STATE_LIMIT {
                    lane.diagnostics.truncated = true;
                    continue;
                }
                if previous.is_none_or(|old| distance < old) {
                    if previous.is_none() {
                        discovered += 1;
                    }
                    lane.distance.insert(target, distance);
                    lane.frontier.push(Arrival {
                        distance,
                        id: graph.node_ids[target],
                        index: target,
                    });
                }
            }
            if mass[f] >= targets[f] {
                break;
            }
        }
        if outer > 0 {
            lane.gain = 0.5 * lane.gain + 0.5 * batch_gain / outer as f64;
        }
    }
    if lanes.iter().any(|l| l.diagnostics.truncated) && reason == "frontier_exhausted" {
        reason = "work_limit";
    }
    for lane in &mut lanes {
        lane.diagnostics.truncated |= !lane.frontier.is_empty() && reason != "mass_target";
    }
    Ok(Supports {
        ids,
        diagnostics: GrowthDiagnostics {
            budget: limit,
            core_nodes: core.len(),
            union_nodes: union.len(),
            overlap_nodes: coverage.values().filter(|set| set.len() > 1).count(),
            settled_states: states,
            scanned_edges: scanned,
            stop_reason: if core.is_empty() {
                "empty_core".into()
            } else {
                reason.into()
            },
            lanes: lanes.into_iter().map(|l| l.diagnostics).collect(),
        },
    })
}
