//! Fact matrix and directed transport construction.
use super::*;

pub(super) fn build_fact_matrix(
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

pub(super) fn build_transport(
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
