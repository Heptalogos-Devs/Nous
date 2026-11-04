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
    sequence: i64,
    #[serde(default = "one")]
    weight: f64,
}
fn one() -> f64 {
    1.0
}
pub struct ContextResult {
    pub vector: Vec<f32>,
    pub diagnostics_json: String,
}
pub struct SegmentResult {
    pub values: Vec<Vec<f32>>,
    pub ranges_json: String,
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
            && raw.as_ref().is_none_or(|v| v.len() == dim)
            && options.window > 0
            && options.window <= 10000,
        "Context shape/window invalid"
    );
    ensure!(
        [options.decay, options.context_mix]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            && options.default_role_weight.is_finite()
            && options.default_role_weight >= 0.0
            && options
                .role_weights
                .values()
                .all(|v| v.is_finite() && *v >= 0.0),
        "Context weights invalid"
    );
    ensure!(
        vectors.iter().all(|v| v.is_finite()),
        "Context contains nonfinite values"
    );
    let mut groups = BTreeMap::<String, Vec<(usize, &Fragment)>>::new();
    let mut seen = BTreeSet::new();
    for (i, f) in fragments.iter().enumerate() {
        ensure!(
            f.weight.is_finite() && f.weight >= 0.0 && !f.key.is_empty() && !f.role.is_empty(),
            "Context fragment invalid"
        );
        if seen.insert(&f.key) {
            groups.entry(f.role.clone()).or_default().push((i, f));
        }
    }
    let mut context = vec![0.0_f64; dim];
    let mut diagnostics = Vec::new();
    for (role, mut fragments) in groups {
        fragments.sort_by_key(|(row, f)| (f.sequence, *row));
        let len = fragments.len();
        let mut sum = vec![0.0_f64; dim];
        let mut used = 0;
        let mut zero = 0;
        for (age, (row, f)) in fragments.into_iter().rev().take(options.window).enumerate() {
            let weight = f.weight * options.decay.powi(age as i32);
            if weight == 0.0 {
                continue;
            }
            match unit(&vectors[row * dim..(row + 1) * dim]) {
                Ok(v) => {
                    for (out, value) in sum.iter_mut().zip(v) {
                        *out += f64::from(value) * weight;
                    }
                    used += 1;
                }
                Err(_) => {
                    zero += 1;
                }
            }
        }
        let weight = options
            .role_weights
            .get(&role)
            .copied()
            .unwrap_or(options.default_role_weight);
        let sum: Vec<f32> = sum.into_iter().map(|v| v as f32).collect();
        let normalized = unit(&sum);
        if let Ok(v) = &normalized {
            for (out, value) in context.iter_mut().zip(v) {
                *out += f64::from(*value) * weight;
            }
        }
        diagnostics.push(json!({"role":role,"supplied":len,"used":used,"zero_norm":zero,"weight":weight,"available":normalized.is_ok()}));
    }
    let context = unit(&context.into_iter().map(|v| v as f32).collect::<Vec<_>>());
    let raw = raw.as_deref().map(unit).transpose()?;
    let (origin, effective) = match (raw, context) {
        (Some(raw), Ok(context)) => {
            let mixed: Vec<f32> = raw
                .iter()
                .zip(&context)
                .map(|(q, c)| {
                    ((1.0 - options.context_mix) * f64::from(*q)
                        + options.context_mix * f64::from(*c)) as f32
                })
                .collect();
            ("raw+context", unit(&mixed)?)
        }
        (Some(raw), Err(_)) => ("raw", raw),
        (None, Ok(context)) => ("context", context),
        (None, Err(_)) => {
            return Err(failure(
                "wave.invalid_input",
                "Query signals have zero total norm",
            ));
        }
    };
    Ok(ContextResult{vector:effective.clone(),diagnostics_json:json!({"query_origin":origin,"roles":diagnostics,"effective_query":effective,"context_mix":options.context_mix}).to_string()})
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
