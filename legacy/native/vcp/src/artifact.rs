//! Native directed artifact computation.
//! Computation consumes typed input and returns typed output.
// Preserve numerical decomposition and floating-point accumulation order.
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

fn evaluate_pair_gains(pairs: &[(i64, i64)], gain: &(impl Fn(i64, i64) -> f64 + Sync)) -> Vec<f64> {
    use rayon::prelude::*;
    pairs.par_iter().map(|&(a, b)| gain(a, b)).collect()
}

mod matrix;
use matrix::{build_fact_matrix, build_transport};
mod provenance;
use provenance::{build_native_artifact, build_provenance};
mod build;
pub use build::build_artifact;

/// Apply the preserved directed transport arithmetic to caller-owned graph facts.
pub fn graph_from_edges(
    edges: &[(i64, i64, f64)],
    anchor_gain: HashMap<i64, f64>,
    config: &Value,
) -> Result<Arc<NativeArtifact>, String> {
    let mut fact: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    for &(source, target, weight) in edges {
        if source <= 0 || target <= 0 || !weight.is_finite() || weight < 0.0 {
            return Err("invalid explicit graph edge".into());
        }
        if weight > 0.0 {
            *fact.entry(source).or_default().entry(target).or_default() += weight;
        }
    }
    let (transport, wormholes) = build_transport(&fact, &anchor_gain, config);
    let mut inbound = HashMap::new();
    let mut sources = transport.keys().copied().collect::<Vec<_>>();
    sources.sort_unstable();
    for source in sources {
        let mut targets = transport[&source].iter().collect::<Vec<_>>();
        targets.sort_by_key(|(target, _)| **target);
        for (target, weight) in targets {
            *inbound.entry(*target).or_default() += *weight;
        }
    }
    Ok(build_native_artifact(
        &transport,
        inbound,
        anchor_gain,
        wormholes,
        &BTreeMap::new(),
    ))
}
