use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
pub struct ReferenceGateTag {
    pub id: i64,
    pub name: String,
    pub contribution: f64,
    pub similarity: f64,
}
#[derive(Debug, Deserialize)]
pub struct ReferenceGateInput {
    pub levels: Vec<Vec<ReferenceGateTag>>,
    pub logic_depth: f64,
    pub entropy: f64,
    pub resonance: f64,
    pub world: String,
    pub activation: f64,
    pub coverage: f64,
    pub core_tags: Vec<String>,
    pub config: ReferenceGateConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ReferenceGateConfig {
    pub base_tag_boost: f64,
    pub activation_multiplier: Vec<f64>,
    pub dynamic_boost_range: Vec<f64>,
    pub core_boost_range: Vec<f64>,
    pub lang_confidence_enabled: bool,
    pub lang_penalty_unknown: f64,
    pub lang_penalty_cross_domain: f64,
    pub layer_decay: f64,
}
impl Default for ReferenceGateConfig {
    fn default() -> Self {
        Self {
            base_tag_boost: 0.6,
            activation_multiplier: vec![0.5, 1.5],
            dynamic_boost_range: vec![0.3, 2.0],
            core_boost_range: vec![1.2, 1.4],
            lang_confidence_enabled: true,
            lang_penalty_unknown: 0.05,
            lang_penalty_cross_domain: 0.2,
            layer_decay: 0.7,
        }
    }
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceGatedTag {
    pub id: i64,
    pub name: String,
    pub weight: f64,
    pub is_core: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceGating {
    pub tags: Vec<ReferenceGatedTag>,
    pub effective_boost: f64,
    pub dynamic_core_boost: f64,
}
fn technical(name: &str, spaces: bool) -> bool {
    !name.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        && name.chars().count() > 3
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '-' | '_' | '.')
                || (spaces && c.is_ascii_whitespace())
        })
}
fn range(values: &[f64], index: usize, fallback: f64) -> f64 {
    values
        .get(index)
        .copied()
        .filter(|v| v.is_finite())
        .unwrap_or(fallback)
}
pub fn reference_gate_tags(input: &ReferenceGateInput) -> ReferenceGating {
    let c = &input.config;
    let activation = range(&c.activation_multiplier, 0, 0.5)
        + input.activation
            * (range(&c.activation_multiplier, 1, 1.5) - range(&c.activation_multiplier, 0, 0.5));
    let dynamic = input.logic_depth * (1.0 + input.resonance.ln_1p()) / (1.0 + input.entropy * 0.5)
        * activation;
    let base = if c.base_tag_boost.is_finite() {
        c.base_tag_boost.max(0.0)
    } else {
        0.0
    };
    let boost = base
        * dynamic.clamp(
            range(&c.dynamic_boost_range, 0, 0.3),
            range(&c.dynamic_boost_range, 1, 2.0),
        );
    let metric = 0.5 * input.logic_depth + 0.5 * (1.0 - input.coverage);
    let core = range(&c.core_boost_range, 0, 1.2)
        + metric * (range(&c.core_boost_range, 1, 1.4) - range(&c.core_boost_range, 0, 1.2));
    let names = input
        .core_tags
        .iter()
        .map(|s| s.to_lowercase())
        .collect::<BTreeSet<_>>();
    let lower = input.world.to_ascii_lowercase();
    let social = ["politics", "society", "history", "economics", "culture"]
        .iter()
        .any(|w| lower.contains(w));
    let technical_world = input.world != "Unknown" && technical(&input.world, false);
    let mut seen = BTreeSet::new();
    let mut tags = Vec::new();
    for (level, items) in input.levels.iter().enumerate() {
        for tag in items {
            if tag.id <= 0 || !seen.insert(tag.id) {
                continue;
            }
            let is_core = names.contains(&tag.name.to_lowercase());
            let relevance = if tag.similarity == 0.0 {
                0.5
            } else {
                tag.similarity
            };
            let core_gain = if is_core {
                core * (0.95 + 0.1 * relevance)
            } else {
                1.0
            };
            let language =
                if c.lang_confidence_enabled && technical(&tag.name, true) && !technical_world {
                    let penalty = if input.world == "Unknown" {
                        c.lang_penalty_unknown
                    } else {
                        c.lang_penalty_cross_domain
                    }
                    .clamp(0.0, 1.0);
                    if social { penalty.sqrt() } else { penalty }
                } else {
                    1.0
                };
            let contribution = if tag.contribution.is_finite() {
                tag.contribution.max(0.0)
            } else {
                0.0
            };
            let weight = contribution
                * c.layer_decay.clamp(0.0, 1.0).powi(level as i32)
                * language
                * core_gain;
            if weight > 0.0 {
                tags.push(ReferenceGatedTag {
                    id: tag.id,
                    name: tag.name.clone(),
                    weight,
                    is_core,
                });
            }
        }
    }
    ReferenceGating {
        tags,
        effective_boost: boost.clamp(0.0, 1.0),
        dynamic_core_boost: core,
    }
}
