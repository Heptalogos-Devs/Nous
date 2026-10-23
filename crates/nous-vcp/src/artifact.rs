//! Native directed artifact computation adapted from memo_artifact_builder.rs.
//! VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53, CC BY-NC-SA 4.0.
//! SQLite loading and publication are replaced by typed input/output.
// Retain the upstream numerical decomposition and floating-point accumulation order.
#![allow(clippy::type_complexity)]

use crate::{assets::TagVectorPool, graph::NativeArtifact};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;
const ALGORITHM_VERSION: &str = "memo.native-artifact-v2-binary";
const V9_ALGORITHM_VERSION: &str = "v9.1-rust-native";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTag {
    pub file_id: i64,
    pub tag_id: i64,
    pub position: i64,
}
#[derive(Debug, Clone)]
struct ProvenanceContribution {
    file_id: i64,
    mass: f64,
}
trait UnitVectors: Sync {
    fn unit_vector(&self, id: i64) -> Option<&[f64]>;
}
impl UnitVectors for HashMap<i64, Vec<f64>> {
    fn unit_vector(&self, id: i64) -> Option<&[f64]> {
        self.get(&id).map(Vec::as_slice)
    }
}
pub struct ArtifactInput {
    pub dimension: usize,
    pub model_sig: String,
    pub generation: String,
    pub geometry_sig: String,
    pub rows: Vec<FileTag>,
    pub tag_vectors: TagVectorPool,
    pub anchor_gain: HashMap<i64, f64>,
    pub config: Value,
}
pub struct ArtifactOutput {
    pub graph: Arc<NativeArtifact>,
    pub metadata: Value,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn hash_text(value: &str, length: usize) -> String {
    let digest = sha256_hex(value.as_bytes());
    digest[..length.min(digest.len())].to_string()
}

fn number(config: &Value, path: &[&str], fallback: f64) -> f64 {
    let mut current = config;
    for key in path {
        let Some(next) = current.get(*key) else {
            return fallback;
        };
        current = next;
    }
    current
        .as_f64()
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn boolean(config: &Value, path: &[&str], fallback: bool) -> bool {
    let mut current = config;
    for key in path {
        let Some(next) = current.get(*key) else {
            return fallback;
        };
        current = next;
    }
    match current {
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_i64().map(|item| item != 0).unwrap_or(fallback),
        _ => fallback,
    }
}

fn stable_sums<K: Ord + Copy>(mut entries: Vec<(K, f64)>) -> Vec<(K, f64)> {
    entries.sort_by_key(|left| left.0);
    let mut write = 0usize;
    for read in 0..entries.len() {
        let (key, value) = entries[read];
        if write > 0 && entries[write - 1].0 == key {
            entries[write - 1].1 += value;
        } else {
            let mut sum = 0.0;
            sum += value;
            entries[write] = (key, sum);
            write += 1;
        }
    }
    entries.truncate(write);
    entries
}

fn add_edge(matrix: &mut Vec<((i64, i64), f64)>, source: i64, target: i64, weight: f64) {
    if source == target || !weight.is_finite() || weight <= 0.0 {
        return;
    }
    matrix.push(((source, target), weight));
}

fn semantic_gain_enabled(config: &Value) -> bool {
    let ordered = config.get("orderedCooccurrence").unwrap_or(&Value::Null);
    boolean(ordered, &["semanticGainEnabled"], false)
        || boolean(ordered, &["semanticGain", "enabled"], false)
}

fn semantic_pairs(rows: &[FileTag]) -> Vec<(i64, i64)> {
    let mut seen = HashSet::new();
    let mut pairs = Vec::new();
    let mut offset = 0;
    while offset < rows.len() {
        let mut end = offset + 1;
        while end < rows.len() && rows[end].file_id == rows[offset].file_id {
            end += 1;
        }
        let tags = &rows[offset..end];
        if (2..=100).contains(&tags.len()) {
            for (i, left) in tags.iter().enumerate() {
                for right in &tags[i + 1..] {
                    let key = (left.tag_id.min(right.tag_id), left.tag_id.max(right.tag_id));
                    if left.tag_id != right.tag_id && seen.insert(key) {
                        pairs.push((left.tag_id, right.tag_id));
                    }
                }
            }
        }
        offset = end;
    }
    pairs
}

fn configured_build_threads(value: Option<&str>, available: usize) -> usize {
    value
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| available.clamp(1, 8))
}

fn evaluate_pair_gains(
    pairs: &[(i64, i64)],
    gain: &(impl Fn(i64, i64) -> f64 + Sync),
    threads: usize,
) -> Vec<f64> {
    if threads > 1
        && !pairs.is_empty()
        && let Ok(pool) = rayon::ThreadPoolBuilder::new().num_threads(threads).build()
    {
        use rayon::prelude::*;
        return pool.install(|| pairs.par_iter().map(|&(a, b)| gain(a, b)).collect());
    }
    pairs.iter().map(|&(a, b)| gain(a, b)).collect()
}

fn build_fact_matrix(
    rows: &[FileTag],
    vectors: &dyn UnitVectors,
    anchor_gain: &HashMap<i64, f64>,
    config: &Value,
) -> std::result::Result<HashMap<i64, HashMap<i64, f64>>, String> {
    let ordered = config.get("orderedCooccurrence").unwrap_or(&Value::Null);
    let forward_gain = number(ordered, &["forwardGain"], 1.0).max(0.0);
    let reverse_gain = number(ordered, &["reverseGain"], 0.42).clamp(
        number(ordered, &["minReverseGain"], 0.25),
        number(ordered, &["maxReverseGain"], 0.70),
    );
    let distance_decay = number(ordered, &["distanceDecay"], 0.0).max(0.0);
    let reverse_guard = number(ordered, &["reverseInversionGuard"], 0.95).clamp(0.0, 1.0);
    let reverse_anchor_enabled = boolean(ordered, &["reverseAnchorBoost"], false);
    let reverse_anchor_max = number(ordered, &["reverseAnchorMax"], 1.5).max(1.0);
    let semantic_enabled = semantic_gain_enabled(config);
    let semantic_peak = number(
        ordered,
        &["semanticGainPeak"],
        number(ordered, &["semanticGain", "peak"], 0.65),
    );
    let semantic_sigma = number(
        ordered,
        &["semanticGainSigma"],
        number(ordered, &["semanticGain", "sigma"], 0.25),
    )
    .max(0.001);
    let low_fallback = number(
        ordered,
        &["semanticGainLowSimFallback"],
        number(ordered, &["semanticGain", "lowSimFallback"], 0.1),
    );

    let semantic_gain = |left: i64, right: i64| -> f64 {
        if !semantic_enabled {
            return 1.0;
        }
        let similarity = vectors
            .unit_vector(left)
            .zip(vectors.unit_vector(right))
            .map(|(a, b)| crate::geometry::dot(a, b).clamp(-1.0, 1.0))
            .unwrap_or(low_fallback);
        if similarity < 0.15 {
            0.4 + similarity * 1.0
        } else {
            0.5 + 0.8
                * (-((similarity - semantic_peak).powi(2))
                    / (2.0 * semantic_sigma * semantic_sigma))
                    .exp()
        }
    };

    // One gain per unordered Tag pair. `dot` multiplies the same two vectors in
    // the same index order whichever comes first, so the stored value has the
    // bits every later file would compute. Bounded by the number of fact pairs.
    let pair_gains: HashMap<(i64, i64), f64> = if semantic_enabled {
        let pairs = semantic_pairs(rows);
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        let threads = configured_build_threads(
            config
                .get("buildThreads")
                .and_then(Value::as_u64)
                .map(|value| value.to_string())
                .as_deref(),
            available,
        );
        let gains = evaluate_pair_gains(&pairs, &semantic_gain, threads);
        pairs
            .into_iter()
            .zip(gains)
            .map(|((a, b), gain)| ((a.min(b), a.max(b)), gain))
            .collect()
    } else {
        HashMap::new()
    };
    let mut matrix = Vec::new();
    let mut offset = 0usize;
    while offset < rows.len() {
        let file_id = rows[offset].file_id;
        let mut end = offset + 1;
        while end < rows.len() && rows[end].file_id == file_id {
            end += 1;
        }
        let tags = &rows[offset..end];
        if tags.len() >= 2 && tags.len() <= 100 {
            for left_index in 0..tags.len() {
                for right_index in (left_index + 1)..tags.len() {
                    let left = &tags[left_index];
                    let right = &tags[right_index];
                    if left.tag_id == right.tag_id {
                        continue;
                    }
                    let semantic = if semantic_enabled {
                        let pair = (left.tag_id.min(right.tag_id), left.tag_id.max(right.tag_id));
                        pair_gains[&pair]
                    } else {
                        1.0
                    };
                    if left.position > 0 && right.position > 0 {
                        let count = tags.len() as f64;
                        let phi_left =
                            0.9 - 0.4 * (left.position - 1).max(0) as f64 / (count - 1.0).max(1.0);
                        let phi_right =
                            0.9 - 0.4 * (right.position - 1).max(0) as f64 / (count - 1.0).max(1.0);
                        let delta = (right.position - left.position).max(1) as f64;
                        let distance = if distance_decay > 0.0 {
                            (-distance_decay * (delta - 1.0)).exp()
                        } else {
                            1.0
                        };
                        let base = phi_left * phi_right * distance;
                        let forward = base * forward_gain * semantic;
                        let mut dynamic_reverse = reverse_gain;
                        if reverse_anchor_enabled {
                            dynamic_reverse *= anchor_gain
                                .get(&left.tag_id)
                                .copied()
                                .unwrap_or(1.0)
                                .min(reverse_anchor_max);
                        }
                        dynamic_reverse = dynamic_reverse.clamp(0.25, 0.70);
                        let backward =
                            (base * dynamic_reverse * semantic).min(forward * reverse_guard);
                        add_edge(&mut matrix, left.tag_id, right.tag_id, forward);
                        add_edge(&mut matrix, right.tag_id, left.tag_id, backward);
                    } else {
                        let weight = 0.7 * 0.7 * semantic;
                        add_edge(&mut matrix, left.tag_id, right.tag_id, weight);
                        add_edge(&mut matrix, right.tag_id, left.tag_id, weight);
                    }
                }
            }
        }
        offset = end;
    }
    let mut output: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    for ((source, target), weight) in stable_sums(matrix) {
        output.entry(source).or_default().insert(target, weight);
    }
    Ok(output)
}

fn build_transport(
    fact: &HashMap<i64, HashMap<i64, f64>>,
    anchor_gain: &HashMap<i64, f64>,
    config: &Value,
) -> (HashMap<i64, HashMap<i64, f64>>, HashSet<(i64, i64)>) {
    let v9 = config.get("v9").unwrap_or(&Value::Null);
    let outbound_mass = number(v9, &["outboundMass"], 0.95).clamp(0.01, 0.999999);
    let reserve_mass = number(v9, &["associationReserveMass"], 0.05).clamp(0.0, outbound_mass);
    let evidence_compression = number(v9, &["evidenceCompression"], 1.0).max(0.01);
    let wormhole_gain = number(v9, &["wormholeGain"], 1.35).max(1.0);
    let tension_threshold = number(v9, &["tensionThreshold"], 1.0).max(0.0);
    let hub_exponent = number(v9, &["hubPenaltyExponent"], 0.3).clamp(0.0, 1.0);
    let hub_floor = number(v9, &["hubPenaltyFloor"], 0.55).clamp(0.05, 1.0);
    let hub_ceiling = number(v9, &["hubPenaltyCeiling"], 1.8).clamp(1.0, 4.0);
    let smoothing_ratio = number(v9, &["hubSmoothingRatio"], 0.1).clamp(0.01, 2.0);

    let mut raw_rows: HashMap<i64, Vec<(i64, f64, bool)>> = HashMap::new();
    let mut target_inflow: HashMap<i64, f64> = HashMap::new();
    let mut sources: Vec<_> = fact.keys().copied().collect();
    sources.sort_unstable();
    for source_id in sources {
        let source = &source_id;
        let mut edges: Vec<_> = fact[source].iter().collect();
        edges.sort_by_key(|(target, _)| **target);
        for (target, compatibility) in edges {
            let evidence = (1.0 + compatibility.max(0.0) * evidence_compression).ln();
            let residual = anchor_gain.get(target).copied().unwrap_or(1.0);
            let wormhole = evidence * residual >= tension_threshold;
            let conductance = evidence * if wormhole { wormhole_gain } else { 1.0 };
            if conductance > 0.0 && conductance.is_finite() {
                raw_rows
                    .entry(*source)
                    .or_default()
                    .push((*target, conductance, wormhole));
                *target_inflow.entry(*target).or_default() += conductance;
            }
        }
    }

    let mut positive: Vec<f64> = target_inflow
        .values()
        .copied()
        .filter(|value| *value > 0.0 && value.is_finite())
        .collect();
    // Linear-time selection of index len/2. `total_cmp` distinguishes every bit
    // pattern, so this is the same value a full sort would place there.
    let median = if positive.is_empty() {
        1.0
    } else {
        let middle = positive.len() / 2;
        *positive
            .select_nth_unstable_by(middle, |left, right| left.total_cmp(right))
            .1
    };
    let smoothing = (median * smoothing_ratio).max(1e-9);
    let hub_penalty = |inflow: f64| -> f64 {
        let relative = inflow / (median + smoothing);
        let raw_penalty = if hub_exponent > 0.0 {
            relative.max(1e-9).powf(-hub_exponent)
        } else {
            1.0
        };
        raw_penalty.clamp(hub_floor, hub_ceiling)
    };
    // The penalty depends only on the target: one `powf` per target, not per edge.
    let penalties: HashMap<i64, f64> = target_inflow
        .iter()
        .map(|(target, inflow)| (*target, hub_penalty(*inflow)))
        .collect();

    let mut transport = HashMap::new();
    let mut wormholes = HashSet::new();
    for (source, edges) in raw_rows {
        let mut adjusted = Vec::new();
        let mut total = 0.0;
        let mut wormhole_total = 0.0;
        for (target, conductance, wormhole) in edges {
            let penalty = penalties
                .get(&target)
                .copied()
                .unwrap_or_else(|| hub_penalty(0.0));
            let value = conductance * penalty;
            if value > 0.0 && value.is_finite() {
                total += value;
                if wormhole {
                    wormhole_total += value;
                }
                adjusted.push((target, value, wormhole));
            }
        }
        if total <= 0.0 {
            continue;
        }
        let reserved = if wormhole_total > 0.0 {
            reserve_mass
        } else {
            0.0
        };
        let main = outbound_mass - reserved;
        let row = transport.entry(source).or_insert_with(HashMap::new);
        for (target, value, wormhole) in adjusted {
            let normalized = main * value / total
                + if wormhole && wormhole_total > 0.0 {
                    reserved * value / wormhole_total
                } else {
                    0.0
                };
            row.insert(target, normalized);
            if wormhole {
                wormholes.insert((source, target));
            }
        }
    }
    (transport, wormholes)
}

fn build_provenance(
    rows: &[FileTag],
    config: &Value,
) -> BTreeMap<(i64, i64), Vec<ProvenanceContribution>> {
    let ordered = config.get("orderedCooccurrence").unwrap_or(&Value::Null);
    let forward_gain = number(ordered, &["forwardGain"], 1.0).max(0.0);
    let reverse_gain = number(ordered, &["reverseGain"], 0.35).max(0.0);
    let distance_decay = number(ordered, &["distanceDecay"], 0.08).max(0.0);
    let mut aggregated: Vec<((i64, i64, i64), f64)> = Vec::new();

    let mut offset = 0usize;
    while offset < rows.len() {
        let file_id = rows[offset].file_id;
        let mut end = offset + 1;
        while end < rows.len() && rows[end].file_id == file_id {
            end += 1;
        }
        let tags = &rows[offset..end];
        if tags.len() >= 2 && tags.len() <= 100 {
            for left_index in 0..tags.len() {
                for right_index in (left_index + 1)..tags.len() {
                    let left = &tags[left_index];
                    let right = &tags[right_index];
                    if left.tag_id == right.tag_id {
                        continue;
                    }
                    let delta = (right.position - left.position).max(1) as f64;
                    let distance = (-distance_decay * (delta - 1.0).max(0.0)).exp();
                    for (source, target, mass) in [
                        (left.tag_id, right.tag_id, forward_gain * distance),
                        (right.tag_id, left.tag_id, reverse_gain * distance),
                    ] {
                        aggregated.push(((source, target, file_id), mass));
                    }
                }
            }
        }
        offset = end;
    }

    let mut output: Vec<((i64, i64), Vec<ProvenanceContribution>)> = Vec::new();
    for ((source, target, file_id), mass) in stable_sums(aggregated) {
        let edge = (source, target);
        if output.last().is_none_or(|(previous, _)| *previous != edge) {
            output.push((edge, Vec::new()));
        }
        output
            .last_mut()
            .unwrap()
            .1
            .push(ProvenanceContribution { file_id, mass });
    }
    output.into_iter().collect()
}

fn build_native_artifact(
    transport: &HashMap<i64, HashMap<i64, f64>>,
    inbound: HashMap<i64, f64>,
    anchor_gain: HashMap<i64, f64>,
    wormholes: HashSet<(i64, i64)>,
    provenance: &BTreeMap<(i64, i64), Vec<ProvenanceContribution>>,
) -> Arc<NativeArtifact> {
    let mut nodes = BTreeSet::new();
    for (source, edges) in transport {
        nodes.insert(*source);
        nodes.extend(edges.keys().copied());
    }
    let node_ids: Vec<i64> = nodes.into_iter().collect();
    let node_index: HashMap<i64, usize> = node_ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect();
    let mut row_offsets = Vec::with_capacity(node_ids.len() + 1);
    let mut targets = Vec::new();
    let mut weights = Vec::new();
    row_offsets.push(0);
    for source in &node_ids {
        let mut edges: Vec<(i64, f64)> = transport
            .get(source)
            .map(|items| items.iter().map(|(id, weight)| (*id, *weight)).collect())
            .unwrap_or_default();
        edges.sort_by_key(|item| item.0);
        for (target, weight) in edges {
            if let Some(index) = node_index.get(&target) {
                targets.push(*index);
                weights.push(weight);
            }
        }
        row_offsets.push(targets.len());
    }
    let native_provenance = provenance
        .iter()
        .map(|(edge, contributions)| {
            (
                *edge,
                contributions
                    .iter()
                    .map(|item| (item.file_id, item.mass))
                    .collect(),
            )
        })
        .collect();
    let max_inbound = inbound.values().copied().fold(0.0, f64::max);
    Arc::new(
        NativeArtifact {
            node_ids,
            node_index,
            row_offsets,
            targets,
            weights,
            inbound,
            max_inbound,
            anchor_gain,
            wormhole_edges: wormholes,
            provenance: native_provenance,
            ..NativeArtifact::default()
        }
        .prepared(),
    )
}

/// Build the same fact/transport/provenance arithmetic from a frozen input snapshot.
pub fn build_artifact(mut input: ArtifactInput) -> Result<ArtifactOutput, String> {
    if input.dimension == 0 || input.model_sig.trim().is_empty() || input.generation.is_empty() {
        return Err("artifact requires dimension, model signature and generation".into());
    }
    if input
        .rows
        .iter()
        .any(|row| row.file_id <= 0 || row.tag_id <= 0)
    {
        return Err("artifact memberships require positive identifiers".into());
    }
    if input
        .anchor_gain
        .iter()
        .any(|(id, gain)| *id <= 0 || !gain.is_finite() || *gain < 0.0)
    {
        return Err(
            "MEMO_ANCHOR_GAIN_INVALID: positive IDs and finite nonnegative gains required".into(),
        );
    }
    input
        .rows
        .sort_by_key(|row| (row.file_id, row.position, row.tag_id));
    let mut vectors = HashMap::new();
    for (id, vector) in &input.tag_vectors {
        if *id <= 0
            || vector.len() != input.dimension
            || vector.iter().any(|value| !value.is_finite())
        {
            return Err(format!("invalid artifact vector {id}"));
        }
        let mut unit: Vec<f64> = vector.iter().map(|value| f64::from(*value)).collect();
        let norm = unit
            .iter()
            .fold(0.0, |sum, value| sum + value * value)
            .sqrt();
        if norm > 1e-12 {
            for value in &mut unit {
                *value /= norm;
            }
            vectors.insert(*id, unit);
        }
    }
    let fact = build_fact_matrix(&input.rows, &vectors, &input.anchor_gain, &input.config)?;
    let (transport, wormholes) = build_transport(&fact, &input.anchor_gain, &input.config);
    let provenance = build_provenance(&input.rows, &input.config);

    let mut inbound = HashMap::new();
    let mut sources: Vec<_> = transport.keys().copied().collect();
    sources.sort_unstable();
    let mut graph_hasher = Sha256::new();
    graph_hasher.update(b"memo-graph-le-v2");
    graph_hasher.update((sources.len() as u64).to_le_bytes());
    for source in sources {
        let mut edges: Vec<_> = transport[&source].iter().collect();
        edges.sort_by_key(|(target, _)| **target);
        graph_hasher.update(source.to_le_bytes());
        graph_hasher.update((edges.len() as u64).to_le_bytes());
        for (target, weight) in edges {
            *inbound.entry(*target).or_default() += *weight;
            graph_hasher.update(target.to_le_bytes());
            graph_hasher.update(weight.to_le_bytes());
        }
    }
    let graph_digest = format!("{:x}", graph_hasher.finalize())[..48].to_owned();
    let mut provenance_hasher = Sha256::new();
    provenance_hasher.update(b"memo-provenance-le-v2");
    provenance_hasher.update((provenance.len() as u64).to_le_bytes());
    for ((source, target), values) in &provenance {
        provenance_hasher.update(source.to_le_bytes());
        provenance_hasher.update(target.to_le_bytes());
        provenance_hasher.update((values.len() as u64).to_le_bytes());
        for value in values {
            provenance_hasher.update(value.file_id.to_le_bytes());
            provenance_hasher.update(value.mass.to_le_bytes());
        }
    }
    let provenance_generation = format!("{:x}", provenance_hasher.finalize())[..40].to_owned();
    let config_json = serde_json::to_string(&input.config)
        .map_err(|error| format!("encode effective config failed: {}", error))?;
    let config_hash = hash_text(&config_json, 32);
    let source_artifact_sig = hash_text(
        &format!(
            "{}|{}|{}|{}",
            V9_ALGORITHM_VERSION, input.model_sig, graph_digest, input.geometry_sig
        ),
        24,
    );
    let artifact_sig = hash_text(
        &format!(
            "{}|{}|{}|{}|{}|{}",
            ALGORITHM_VERSION,
            source_artifact_sig,
            graph_digest,
            provenance_generation,
            input.generation,
            config_hash
        ),
        48,
    );

    let native = build_native_artifact(
        &transport,
        inbound,
        input.anchor_gain,
        wormholes,
        &provenance,
    );
    drop(transport);

    let metadata = json!({"algorithmVersion":ALGORITHM_VERSION,"artifactSig":artifact_sig,
        "sourceArtifactSig":source_artifact_sig,"graphGeneration":graph_digest,
        "provenanceGeneration":provenance_generation,"generation":input.generation,
        "modelSig":input.model_sig,"configHash":config_hash,"maxInbound":native.max_inbound,
        "effectiveConfig":input.config});
    Ok(ArtifactOutput {
        graph: native,
        metadata,
    })
}
