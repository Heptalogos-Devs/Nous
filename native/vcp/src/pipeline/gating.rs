//! Lexical focus and conservative graph seed admission.
use super::*;
#[derive(Clone)]
pub(super) struct GatedTag {
    pub id: i64,
    pub name: String,
    pub weight: f64,
    pub is_core: bool,
    pub lexical: bool,
}
pub(super) fn lexical_tokens(text: &str) -> HashSet<String> {
    let mut tokens = HashSet::new();
    let mut ascii = String::new();
    let mut cjk = Vec::new();
    let flush = |ascii: &mut String, cjk: &mut Vec<char>, tokens: &mut HashSet<String>| {
        if !ascii.is_empty() {
            tokens.insert(std::mem::take(ascii));
        }
        if cjk.len() == 1 {
            tokens.insert(cjk[0].to_string());
        }
        for pair in cjk.windows(2) {
            tokens.insert(pair.iter().collect());
        }
        cjk.clear();
    };
    for ch in text.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            if !cjk.is_empty() {
                flush(&mut ascii, &mut cjk, &mut tokens);
            }
            ascii.push(ch);
        } else if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            if !ascii.is_empty() {
                flush(&mut ascii, &mut cjk, &mut tokens);
            }
            cjk.push(ch);
        } else {
            flush(&mut ascii, &mut cjk, &mut tokens);
        }
    }
    flush(&mut ascii, &mut cjk, &mut tokens);
    tokens
}
pub(super) fn lexical_grounding(query: &str, name: &str) -> f64 {
    let query = lexical_tokens(query);
    let tag = lexical_tokens(name);
    if tag.is_empty() {
        return 0.0;
    }
    tag.intersection(&query).count() as f64 / tag.len() as f64
}
pub(super) fn range_value(values: &[f64], index: usize, fallback: f64) -> f64 {
    values
        .get(index)
        .copied()
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}
pub(super) fn contains_cjk(value: &str) -> bool {
    value
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
}
pub(super) fn technical_name(value: &str, allow_space: bool) -> bool {
    !contains_cjk(value)
        && value.chars().count() > 3
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_' | '.')
                || (allow_space && character.is_ascii_whitespace())
        })
}
pub(super) fn scale_implicit_seeds(gated: &mut [GatedTag], eligible: Option<&HashMap<i64, usize>>) {
    let explicit_max = gated
        .iter()
        .filter(|t| {
            (t.is_core || t.lexical) && eligible.map_or(true, |ids| ids.contains_key(&t.id))
        })
        .map(|t| t.weight)
        .fold(0.0, f64::max);
    let implicit_max = gated
        .iter()
        .filter(|t| !t.is_core && !t.lexical)
        .map(|t| t.weight)
        .fold(0.0, f64::max);
    if explicit_max > 0.0 && implicit_max > 0.35 * explicit_max {
        let scale = 0.35 * explicit_max / implicit_max;
        for tag in gated {
            if !tag.is_core && !tag.lexical {
                tag.weight *= scale;
            }
        }
    }
}
pub(super) fn gate_tags_for_graph(
    levels: &[Vec<PyramidTagWork>],
    epa: &EpaOutput,
    pyramid: &PyramidOutput,
    _core_tags: &[String],
    config: &PipelineConfig,
    query_text: &str,
    eligible: Option<&HashMap<i64, usize>>,
    explicit_cores: &[(i64, String)],
) -> (Vec<GatedTag>, f64, f64) {
    let activation_min = range_value(&config.activation_multiplier, 0, 0.5);
    let activation_max = range_value(&config.activation_multiplier, 1, 1.5);
    let activation_multiplier =
        activation_min + pyramid.features.activation * (activation_max - activation_min);
    let dynamic_factor = epa.logic_depth * (1.0 + epa.resonance.ln_1p())
        / (1.0 + epa.entropy * 0.5)
        * activation_multiplier;
    let boost_min = range_value(&config.dynamic_boost_range, 0, 0.3);
    let boost_max = range_value(&config.dynamic_boost_range, 1, 2.0);
    let effective_boost =
        positive(config.base_tag_boost) * dynamic_factor.clamp(boost_min, boost_max);

    let core_metric = epa.logic_depth * 0.5 + (1.0 - pyramid.features.coverage) * 0.5;
    let core_min = range_value(&config.core_boost_range, 0, 1.2);
    let core_max = range_value(&config.core_boost_range, 1, 1.4);
    let dynamic_core_boost = core_min + core_metric * (core_max - core_min);
    let world = epa
        .dominant_axes
        .first()
        .map(|axis| axis.label.as_str())
        .unwrap_or("Unknown");
    let world_lower = world.to_ascii_lowercase();
    let social_world = ["politics", "society", "history", "economics", "culture"]
        .iter()
        .any(|token| world_lower.contains(token));
    let technical_world = world != "Unknown" && technical_name(world, false);

    let mut seen = HashSet::new();
    let mut gated = Vec::new();
    for (level, tags) in levels.iter().enumerate() {
        let maximum = tags
            .iter()
            .map(|tag| positive(tag.contribution))
            .fold(0.0, f64::max);
        let threshold = (maximum * config.seed_contribution_ratio.clamp(0.0, 1.0))
            .max(config.seed_contribution_floor.max(0.0));
        let dominant = tags
            .first()
            .filter(|tag| tag.contribution.powi(2) > 0.85 && tag.similarity > 0.0);
        // At most two redundant directions borrow amplitude; the sum is bounded.
        // Raw orthogonal contributions and residuals remain untouched.
        let mut redundancy_left = dominant.map_or(0.0, |tag| 0.3 * tag.contribution);
        let mut redundancy_slots = 2usize;
        for tag in tags {
            let is_core = explicit_cores.iter().any(|(id, _)| *id == tag.id);
            if tag.id <= 0 || seen.contains(&tag.id) {
                continue;
            }
            let grounding = tag
                .aliases
                .iter()
                .map(|alias| lexical_grounding(query_text, alias))
                .fold(lexical_grounding(query_text, &tag.name), f64::max);
            let lexical = grounding >= 0.5;
            // Only already independently supported peers can lend amplitude.
            // A low-contribution alias cannot recursively sponsor other aliases.
            let sponsor = tags
                .iter()
                .filter(|peer| {
                    peer.id != tag.id && peer.contribution >= threshold && peer.similarity > 0.0
                })
                .filter_map(|peer| {
                    let tightness = clamp01(cosine(&peer.vector, &tag.vector));
                    (tightness >= 0.80).then_some(peer.contribution * tightness)
                })
                .fold(0.0, f64::max);
            let alias = tag.similarity >= 0.20 && sponsor > 0.0;
            let collapsed_redundant = !is_core
                && !lexical
                && tag.contribution < 1e-6
                && tag.similarity >= 0.75
                && dominant.is_some_and(|first| {
                    first.id != tag.id && cosine(&first.vector, &tag.vector) >= 0.90
                });
            let redundant = collapsed_redundant && redundancy_slots > 0;
            if collapsed_redundant && !redundant {
                continue;
            }
            let redundancy_floor = if redundant {
                let first = dominant.unwrap();
                let borrowed = (0.15
                    * first.contribution
                    * clamp01(tag.similarity / first.similarity)
                    * clamp01(cosine(&first.vector, &tag.vector)))
                .min(redundancy_left);
                redundancy_left -= borrowed;
                redundancy_slots -= 1;
                borrowed
            } else {
                0.0
            };
            if !is_core
                && !lexical
                && !alias
                && (tag.contribution < threshold || tag.similarity <= 0.0)
            {
                continue;
            }
            let effective_contribution = if lexical {
                // Preserve 1.1 literal-focus ignition independently of alias policy.
                tag.contribution.max(0.6 * sponsor).max(0.6 * maximum)
            } else if redundant {
                tag.contribution.max(redundancy_floor)
            } else if alias {
                tag.contribution
                    .max(0.6 * sponsor * tag.similarity.clamp(0.2, 1.0))
            } else {
                tag.contribution
            };
            seen.insert(tag.id);
            // Cosine zero is a measured orthogonal relationship, not a
            // missing-score sentinel. Every pyramid candidate is measured
            // against its original frozen vector before entering this gate.
            let relevance = tag.similarity;
            let core_gain = if is_core {
                dynamic_core_boost * (0.95 + relevance * 0.1)
            } else {
                1.0
            };
            let mut language_gain = 1.0;
            if !lexical
                && config.lang_confidence_enabled
                && technical_name(&tag.name, true)
                && !technical_world
            {
                let penalty = if world == "Unknown" {
                    config.lang_penalty_unknown
                } else {
                    config.lang_penalty_cross_domain
                }
                .clamp(0.0, 1.0);
                language_gain = if social_world {
                    penalty.sqrt()
                } else {
                    penalty
                };
            }
            let decay = config.layer_decay.clamp(0.0, 1.0).powi(level as i32);
            // Restore, never amplify, strong residual foci in accepted layers.
            let independent_support = clamp01(effective_contribution / maximum.max(1e-12));
            let layer_gain = if lexical {
                1.0
            } else {
                decay + (1.0 - decay) * independent_support
            };
            let raw_weight = positive(effective_contribution)
                * layer_gain
                * language_gain
                * core_gain
                * if lexical && !is_core { 1.6 } else { 1.0 };
            let weight = if is_core {
                raw_weight
            } else {
                raw_weight.min(1.0)
            };
            if weight > 0.0 {
                gated.push(GatedTag {
                    id: tag.id,
                    name: tag.name.clone(),
                    weight,
                    is_core,
                    lexical,
                });
            }
        }
    }
    // Explicit focus has unit base amplitude, independent of ANN recall and
    // Gram-Schmidt contribution. Promote existing IDs instead of duplicating them.
    for (id, name) in explicit_cores {
        if let Some(tag) = gated.iter_mut().find(|tag| tag.id == *id) {
            tag.is_core = true;
            tag.weight = tag.weight.max(dynamic_core_boost);
        } else {
            gated.push(GatedTag {
                id: *id,
                name: name.clone(),
                weight: dynamic_core_boost,
                is_core: true,
                lexical: false,
            });
        }
    }
    scale_implicit_seeds(&mut gated, eligible);
    (gated, effective_boost.clamp(0.0, 1.0), dynamic_core_boost)
}
