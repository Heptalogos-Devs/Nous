//! Query context aggregation and adjacent-vector segmentation.
use crate::{failure, snapshot::unit};
use anyhow::{Result, ensure};
use nous_vcp::config::ContextControls;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Deserialize)]
struct Fragment {
    key: String,
    role: String,
    #[serde(default = "retrieval")]
    stage: String,
    parent_key: Option<String>,
    sequence: i64,
    #[serde(default = "one")]
    weight: f64,
    #[serde(default = "one")]
    relevance: f64,
    #[serde(default = "one")]
    validity: f64,
    #[serde(default)]
    distance: f64,
    half_life: Option<f64>,
    source_key: Option<String>,
    source_start: Option<u64>,
    source_end: Option<u64>,
}
fn one() -> f64 {
    1.0
}
fn retrieval() -> String {
    "retrieval".into()
}
pub struct ContextResult {
    pub vector: Vec<f32>,
    pub diagnostics_json: String,
}
pub struct SegmentResult {
    pub values: Vec<Vec<f32>>,
    pub ranges_json: String,
}

/// Amount of a source range not already represented by this parent.
fn additional_coverage(ranges: &mut Vec<(u64, u64)>, start: u64, end: u64) -> f64 {
    if end <= start {
        return 0.0;
    }
    ranges.sort_unstable();
    let mut cursor = start;
    let mut uncovered = 0_u64;
    for &(left, right) in ranges.iter() {
        if right <= cursor || left >= end {
            continue;
        }
        if left > cursor {
            uncovered += left.min(end) - cursor;
        }
        cursor = cursor.max(right).min(end);
    }
    if cursor < end {
        uncovered += end - cursor;
    }
    ranges.push((start, end));
    uncovered as f64
}

#[derive(serde::Serialize)]
struct Allocation {
    row: usize,
    key: String,
    role: String,
    stage: String,
    parent_key: String,
    contribution: f64,
    decayed_weight: f64,
    role_weight: f64,
}

fn allocate(
    fragments: &[Fragment],
    options: &ContextControls,
    valid: Option<&[bool]>,
) -> Result<(Vec<Allocation>, Vec<Value>)> {
    ensure!(
        (1..=10000).contains(&options.window)
            && options.context_mix.is_finite()
            && (0.0..=1.0).contains(&options.context_mix)
            && options.null_mass.is_finite()
            && options.null_mass > 0.0
            && options.default_role_weight.is_finite()
            && options.default_role_weight >= 0.0
            && options
                .role_weights
                .values()
                .all(|v| v.is_finite() && *v >= 0.0),
        "Context weights invalid"
    );
    type Parent<'a> = Vec<(usize, &'a Fragment)>;
    let mut groups = BTreeMap::<(String, String), BTreeMap<String, Parent<'_>>>::new();
    let mut seen = BTreeSet::new();
    for (i, fragment) in fragments.iter().enumerate() {
        ensure!(
            !fragment.key.is_empty()
                && !fragment.role.is_empty()
                && !fragment.stage.is_empty()
                && fragment.weight.is_finite()
                && fragment.weight >= 0.0
                && fragment.relevance.is_finite()
                && (0.0..=1.0).contains(&fragment.relevance)
                && fragment.validity.is_finite()
                && (0.0..=1.0).contains(&fragment.validity)
                && fragment.distance.is_finite()
                && fragment.distance >= 0.0
                && fragment.half_life.is_none_or(|v| v.is_finite() && v > 0.0),
            "Context fragment invalid"
        );
        if valid.is_none_or(|rows| rows[i]) && seen.insert(&fragment.key) {
            groups
                .entry((fragment.stage.clone(), fragment.role.clone()))
                .or_default()
                .entry(
                    fragment
                        .parent_key
                        .clone()
                        .unwrap_or_else(|| fragment.key.clone()),
                )
                .or_default()
                .push((i, fragment));
        }
    }
    let mut diagnostics = Vec::new();
    let mut selected = Vec::new();
    for ((stage, role), parents) in groups {
        let mut parents = parents.into_iter().collect::<Vec<_>>();
        parents.sort_by_key(|(_, parts)| {
            std::cmp::Reverse(
                parts
                    .iter()
                    .map(|(_, f)| f.sequence)
                    .max()
                    .unwrap_or_default(),
            )
        });
        let supplied = parents.len();
        parents.truncate(options.window);
        let mut prepared = Vec::new();
        for (key, parts) in parents {
            let mut ranges = BTreeMap::<String, Vec<(u64, u64)>>::new();
            let mut unique = BTreeSet::new();
            let mut samples = Vec::new();
            let mut total = 0.0;
            let mut utility = 0.0;
            for (row, fragment) in parts {
                let source = fragment.source_key.as_deref().unwrap_or(&fragment.key);
                let coverage = match (fragment.source_start, fragment.source_end) {
                    (Some(start), Some(end)) => {
                        additional_coverage(ranges.entry(source.into()).or_default(), start, end)
                    }
                    _ if unique.insert(source.to_string()) => 1.0,
                    _ => 0.0,
                };
                if coverage == 0.0 {
                    continue;
                }
                let decay = fragment
                    .half_life
                    .map_or(1.0, |half| 2.0_f64.powf(-fragment.distance / half));
                let value = fragment.relevance * fragment.validity * fragment.weight * decay;
                total += coverage;
                utility += coverage * value;
                samples.push((row, fragment, coverage, value));
            }
            if total > 0.0 {
                prepared.push((key, utility / total, total, samples));
            }
        }
        let denominator = options.null_mass + prepared.iter().map(|(_, u, _, _)| u).sum::<f64>();
        let budget = options.context_mix
            * options
                .role_weights
                .get(&role)
                .copied()
                .unwrap_or(options.default_role_weight);
        let mut total_mass = 0.0;
        for (parent, utility, coverage, samples) in prepared {
            let mass = budget * utility / denominator;
            total_mass += mass;
            for (row, fragment, covered, value) in samples {
                let contribution = mass * covered / coverage;
                if contribution == 0.0 {
                    continue;
                }
                selected.push(Allocation {
                    row,
                    key: fragment.key.clone(),
                    role: role.clone(),
                    stage: stage.clone(),
                    parent_key: parent.clone(),
                    contribution,
                    decayed_weight: value,
                    role_weight: budget,
                });
            }
        }
        diagnostics.push(json!({"role":role,"stage":stage,"supplied":supplied,
            "budget":budget,"contribution":total_mass,"null_mass":options.null_mass}));
    }
    Ok((selected, diagnostics))
}

/// Calculate the same parent masses for lexical and vector context branches.
pub fn context_masses(metadata_json: &str, options_json: &str) -> Result<String> {
    let fragments: Vec<Fragment> = serde_json::from_str(metadata_json)?;
    let options: ContextControls = serde_json::from_str(options_json)?;
    let (selected, roles) = allocate(&fragments, &options, None)?;
    Ok(json!({"selected_context":selected,"roles":roles}).to_string())
}

pub fn aggregate_context(
    raw: Option<Vec<f32>>,
    vectors: Vec<f32>,
    rows: usize,
    dim: usize,
    metadata_json: &str,
    options_json: &str,
) -> Result<ContextResult> {
    let fragments: Vec<Fragment> = serde_json::from_str(metadata_json)?;
    let options: ContextControls = serde_json::from_str(options_json)?;
    ensure!(
        dim > 0
            && rows == fragments.len()
            && vectors.len() == rows * dim
            && raw.as_ref().is_none_or(|v| v.len() == dim),
        "Context shape invalid"
    );
    ensure!(
        vectors.iter().all(|v| v.is_finite()),
        "Context contains nonfinite values"
    );
    let units = (0..rows)
        .map(|row| unit(&vectors[row * dim..(row + 1) * dim]).ok())
        .collect::<Vec<_>>();
    let valid = units.iter().map(Option::is_some).collect::<Vec<_>>();
    let (selected, roles) = allocate(&fragments, &options, Some(&valid))?;
    let mut context = vec![0.0_f64; dim];
    for item in &selected {
        for (out, value) in context.iter_mut().zip(units[item.row].as_ref().unwrap()) {
            *out += f64::from(*value) * item.contribution;
        }
    }
    let raw = raw.as_deref().map(unit).transpose()?;
    let has_context = context.iter().any(|v| *v != 0.0);
    let (origin, combined) = if let Some(raw) = raw {
        let combined = raw
            .iter()
            .zip(context)
            .map(|(q, c)| (f64::from(*q) + c) as f32)
            .collect::<Vec<_>>();
        (if has_context { "raw+context" } else { "raw" }, combined)
    } else if has_context {
        ("context", context.into_iter().map(|v| v as f32).collect())
    } else {
        return Err(failure(
            "wave.invalid_input",
            "Query signals have zero total norm",
        ));
    };
    let effective = unit(&combined)?;
    Ok(ContextResult {
        vector: effective.clone(),
        diagnostics_json: json!({
            "query_origin":origin,"roles":roles,"selected_context":selected,
            "effective_query":effective,"context_mix":options.context_mix
        })
        .to_string(),
    })
}
pub fn segment(values: Vec<f32>, n: usize, d: usize, threshold: f64) -> Result<SegmentResult> {
    ensure!(
        d > 0 && threshold.is_finite() && (-1.0..=1.0).contains(&threshold),
        "Segment shape or threshold invalid"
    );
    let units = (0..n)
        .map(|r| unit(&values[r * d..(r + 1) * d]))
        .collect::<Result<Vec<_>>>()?;
    let mut boundaries = vec![0];
    for i in 1..n {
        let cosine = units[i - 1]
            .iter()
            .zip(&units[i])
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum::<f64>();
        if cosine < threshold {
            boundaries.push(i);
        }
    }
    if n > 0 {
        boundaries.push(n);
    }
    let mut outputs = Vec::new();
    let mut ranges = Vec::<Value>::new();
    for pair in boundaries.windows(2) {
        let mut mean = vec![0.0_f32; d];
        for vector in &units[pair[0]..pair[1]] {
            for (out, v) in mean.iter_mut().zip(vector) {
                *out += *v;
            }
        }
        outputs.push(unit(&mean)?);
        ranges.push(json!({"start":pair[0],"end":pair[1]}));
    }
    Ok(SegmentResult {
        values: outputs,
        ranges_json: json!(ranges).to_string(),
    })
}
