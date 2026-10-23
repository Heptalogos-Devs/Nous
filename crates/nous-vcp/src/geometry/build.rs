//! Local and global geometry construction from explicit memberships.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

/// Compute upstream local/global AGS and EPA assets without reading storage.
pub fn build_geometry(mut input: GeometryInput) -> Result<GeometryOutput, String> {
    if input.dimension == 0
        || input.model_sig.trim().is_empty()
        || input.tags.iter().any(|tag| tag.id <= 0)
    {
        return Err("geometry requires dimension, model signature and positive Tag ids".into());
    }
    input.tags.sort_by_key(|tag| tag.id);
    if input.tags.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("duplicate geometry Tag".into());
    }
    let declared: std::collections::HashSet<i64> = input.tags.iter().map(|tag| tag.id).collect();
    if input
        .memberships
        .iter()
        .any(|row| row.file_id <= 0 || !declared.contains(&row.tag_id))
    {
        return Err("MEMO_GEOMETRY_MEMBERSHIP_INVALID: memberships require positive files and declared Tags".into());
    }
    input
        .memberships
        .sort_by_key(|row| (row.file_id, row.position, row.tag_id));
    let config = &input.config;
    let local = config.get("local").unwrap_or(config);
    let global = config.get("global").unwrap_or(&Value::Null);
    let max_neighbors = number(local, "maxNeighbors", 48.0, 1.0, 256.0) as usize;
    let min_neighbors = number(local, "minNeighbors", 3.0, 1.0, 256.0) as usize;
    let max_basis = number(local, "maxBasis", 4.0, 1.0, 32.0) as usize;
    let min_gain = number(local, "minGain", 0.015, 0.0, 1.0);
    let decay = number(local, "positionDecay", 0.15, 0.0, 4.0);
    let k_max = number(global, "kMax", 32.0, 1.0, 128.0) as usize;
    let k_sem = number(global, "kSem", 8.0, 1.0, k_max as f64) as usize;
    let mut signature_config = config.clone();
    if let Some(object) = signature_config.as_object_mut() {
        object.remove("forceRecompute");
    }
    let config_hash = hash(signature_config.to_string().as_bytes());
    let mut digest = Sha256::new();
    let mut adjacency: BTreeMap<i64, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut ids = Vec::new();
    let mut names = HashMap::new();
    for tag in &input.tags {
        digest.update(tag.id.to_le_bytes());
        digest.update(tag.vector_version.to_le_bytes());
        digest.update((tag.name.len() as u64).to_le_bytes());
        digest.update(tag.name.as_bytes());
        ids.push(tag.id);
        names.insert(tag.id, tag.name.clone());
    }
    let flush = |tags: &[(i64, i64)], graph: &mut BTreeMap<i64, BTreeMap<i64, f64>>| {
        if !(2..=100).contains(&tags.len()) {
            return;
        }
        for i in 0..tags.len() {
            for j in i + 1..tags.len() {
                let (a, p) = tags[i];
                let (b, q) = tags[j];
                if a == b {
                    continue;
                }
                let delta = if p > 0 && q > 0 {
                    (p - q).abs().max(1) as f64
                } else {
                    1.0
                };
                let weight = (-decay * (delta - 1.0)).exp();
                *graph.entry(a).or_default().entry(b).or_default() += weight;
                *graph.entry(b).or_default().entry(a).or_default() += weight;
            }
        }
    };

    let mut file = None;
    let mut tags = Vec::new();
    for row in &input.memberships {
        let (fid, id, pos) = (row.file_id, row.tag_id, row.position);
        digest.update(fid.to_le_bytes());
        digest.update(id.to_le_bytes());
        digest.update(pos.to_le_bytes());
        if file != Some(fid) {
            flush(&tags, &mut adjacency);
            tags.clear();
            file = Some(fid);
        }
        if tags.len() <= 100 {
            tags.push((id, pos));
        }
    }
    flush(&tags, &mut adjacency);
    let content = format!("{:x}", digest.finalize());
    let signature = hash(
        format!(
            "{}|{}|{}|{}|{}",
            ALGORITHM, input.dimension, input.model_sig, config_hash, content
        )
        .as_bytes(),
    );
    let vectors = UnitVectorWorkspace::build(&input.tags, input.dimension)?;
    let star_neighbors = number(global, "starNeighbors", max_neighbors as f64, 1.0, 256.0) as usize;
    let max_keep = max_neighbors.max(star_neighbors);
    // Pre-truncated, pre-sorted Slot-CSR: filter invalid vectors and truncate to `max_keep` once!
    let mut offsets = vec![0];
    let mut edges: Vec<(i64, usize, f64)> = Vec::new();
    let mut support = Vec::with_capacity(ids.len());
    let cmp_edge =
        |a: &(i64, usize, f64), b: &(i64, usize, f64)| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0));
    for id in &ids {
        let neighbors = adjacency.remove(id).unwrap_or_default();
        support.push(neighbors.values().sum::<f64>());
        if vectors.contains_key(id) {
            let mut valid_edges: Vec<(i64, usize, f64)> = neighbors
                .into_iter()
                .filter_map(|(nid, w)| vectors.slot(&nid).map(|slot| (nid, slot, w)))
                .collect();
            if valid_edges.len() > max_keep {
                valid_edges.select_nth_unstable_by(max_keep - 1, cmp_edge);
                valid_edges.truncate(max_keep);
            }
            valid_edges.sort_unstable_by(cmp_edge);
            edges.extend(valid_edges);
        }
        offsets.push(edges.len());
    }
    drop(adjacency);
    let gate = local.get("semanticGate").unwrap_or(local);
    let gate_enabled = gate
        .get("enabled")
        .or_else(|| gate.get("semanticEnabled"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let peak = number(
        gate,
        "peak",
        number(local, "semanticPeak", 0.65, -1.0, 1.0),
        -1.0,
        1.0,
    );
    let sigma = number(
        gate,
        "sigma",
        number(local, "semanticSigma", 0.25, 0.02, 2.0),
        0.02,
        2.0,
    );
    let floor = number(
        gate,
        "floor",
        number(local, "semanticFloor", 0.35, 0.0, 1.0),
        0.0,
        1.0,
    );
    let hard_floor = number(
        gate,
        "hardFloor",
        number(local, "semanticHardFloor", -1.0, -1.0, 1.0),
        -1.0,
        1.0,
    );
    let results: Vec<LocalResult> = ids
        .par_iter()
        .enumerate()
        .map(|(i, id)| {
            let mut result = LocalResult {
                id: *id,
                status: "failed",
                count: 0,
                residual: 1.0,
            };
            let Some(target_slot) = vectors.slot(id) else {
                return result;
            };
            let target = vectors.get_by_slot(target_slot);
            let end = offsets[i + 1].min(offsets[i] + max_neighbors);
            let neighbors = &edges[offsets[i]..end];
            let neighbor_vecs: Vec<&[f64]> = neighbors
                .iter()
                .map(|&(_, slot, _)| vectors.get_by_slot(slot))
                .collect();
            let raw_dots = dot_neighbors(target, &neighbor_vecs);
            let mut candidates = Vec::with_capacity(neighbors.len());
            let mut refs = Vec::with_capacity(neighbors.len());
            let mut diagonal = Vec::with_capacity(neighbors.len());
            let mut correlations = Vec::with_capacity(neighbors.len());
            for (&(nid, slot, w), (&v, raw_dot)) in
                neighbors.iter().zip(neighbor_vecs.iter().zip(raw_dots))
            {
                let sim = raw_dot.clamp(-1.0, 1.0);
                let semantic = semantic_weight(sim, gate_enabled, peak, sigma, floor, hard_floor);
                if semantic > 0.0 {
                    candidates.push(AgsCandidate {
                        id: nid,
                        prior: w.ln_1p() * semantic,
                    });
                    refs.push(v);
                    diagonal.push(vectors.norm_by_slot(slot));
                    correlations.push(raw_dot);
                }
            }
            result.count = candidates.len();
            if result.count < min_neighbors {
                result.status = "insufficient_neighbors";
                return result;
            }
            let trace = direct_axes_core(
                AgsTarget::Explain(target),
                &candidates,
                &refs,
                Some(&diagonal),
                Some(&correlations),
                max_basis,
                min_gain,
                false,
            );
            result.residual = trace.residual_norm.unwrap_or(1.0).clamp(0.0, 1.0);
            result.status = "computed";
            result
        })
        .collect();

    let mut mean = vec![0.0; input.dimension];
    for slot in 0..vectors.len() {
        let v = vectors.get_by_slot(slot);
        for (m, x) in mean.iter_mut().zip(v) {
            *m += x;
        }
    }
    if !vectors.is_empty() {
        for m in &mut mean {
            *m /= vectors.len() as f64;
        }
    }
    let min_support = number(global, "minSupport", 3.0, 0.0, 1e12);
    let a = number(global, "supportExponent", 0.65, 0.0, 4.0);
    let b = number(global, "residualExponent", 0.35, 0.0, 4.0);
    let prior_mode = global
        .get("priorMode")
        .and_then(Value::as_str)
        .unwrap_or("support");
    if !["support", "explained", "residual"].contains(&prior_mode) {
        return Err("geometry global.priorMode must be support, explained or residual".into());
    }
    let candidate_mode = global
        .get("candidateMode")
        .and_then(Value::as_str)
        .unwrap_or("star");
    if !["star", "anchor"].contains(&candidate_mode) {
        return Err("geometry global.candidateMode must be star or anchor".into());
    }
    let mut pool: Vec<_> = results
        .iter()
        .enumerate()
        .filter(|(i, r)| r.status == "computed" && support[*i] >= min_support)
        .map(|(i, r)| {
            let factor = match prior_mode {
                "explained" => (1.0 - r.residual * r.residual).max(0.0).powf(b),
                "residual" => r.residual.powf(b),
                _ => 1.0,
            };
            AgsCandidate {
                id: r.id,
                prior: support[i].powf(a) * factor,
            }
        })
        .collect();
    pool.sort_by(|a, b| b.prior.total_cmp(&a.prior).then(a.id.cmp(&b.id)));
    pool.truncate(number(global, "candidatePool", 512.0, 1.0, 4096.0) as usize);
    let mix = number(global, "starMix", 0.8, 0.0, 1.0);
    let centered: Vec<Vec<f64>> = pool
        .par_iter()
        .map(|c| {
            let target = &vectors[&c.id];
            let i = ids.binary_search(&c.id).unwrap();
            let end = offsets[i + 1].min(offsets[i] + star_neighbors);
            let neighbors = &edges[offsets[i]..end];
            let neighbor_vecs: Vec<&[f64]> = neighbors
                .iter()
                .map(|&(_, slot, _)| vectors.get_by_slot(slot))
                .collect();
            let raw_dots = dot_neighbors(target, &neighbor_vecs);
            let weighted: Vec<_> = neighbors
                .iter()
                .zip(neighbor_vecs.iter().zip(raw_dots))
                .filter_map(|(&(_, _, w), (&v, raw_dot))| {
                    let weight = w.ln_1p()
                        * semantic_weight(
                            raw_dot.clamp(-1.0, 1.0),
                            gate_enabled,
                            peak,
                            sigma,
                            floor,
                            hard_floor,
                        );
                    (weight > 0.0).then_some((v, weight))
                })
                .collect();
            star_direction(
                target,
                &mean,
                &weighted,
                if candidate_mode == "star" { mix } else { 0.0 },
            )
        })
        .collect();
    let refs: Vec<_> = centered.iter().map(Vec::as_slice).collect();
    let mut trace = anchored_gs(
        AgsTarget::Span,
        &pool,
        &refs,
        k_max,
        number(global, "minGain", 0.02, 0.0, 1.0),
    );
    drop(refs);
    drop(centered);
    drop(pool);
    drop(edges);
    drop(offsets);
    drop(support);

    // Fixed-size chunks + indexed collect + serial reduction = thread-count-independent bits.
    let partial: Vec<_> = ids
        .par_chunks(128)
        .map(|batch| {
            let mut energy = 0.0;
            let mut captured = vec![0.0; trace.basis.len()];
            let mut x = vec![0.0; input.dimension];
            for id in batch {
                if let Some(v) = vectors.get(id) {
                    for ((out, &val), &m) in x.iter_mut().zip(v).zip(&mean) {
                        *out = val - m;
                    }
                    energy += dot(&x, &x);
                    accumulate_captured_energy(&x, &trace.basis, &mut captured);
                }
            }
            (energy, captured)
        })
        .collect();
    drop(vectors);
    let mut total = 0.0;
    let mut energies = vec![0.0; trace.basis.len()];
    for (energy, values) in partial {
        total += energy;
        for (e, v) in energies.iter_mut().zip(values) {
            *e += v;
        }
    }
    let mut cumulative = 0.0;
    let mut coverage: Vec<_> = energies
        .iter()
        .map(|e| {
            cumulative += e;
            if total > 1e-12 {
                (cumulative / total).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();
    let full_coverage = coverage.clone();
    let k = effective_k(
        &coverage,
        k_sem,
        number(global, "epsilon", 0.1, 0.0, 1.0),
        number(global, "kneeDelta", 0.0, 0.0, 1.0),
    );
    trace.basis.truncate(k);
    trace.picked.truncate(k);
    energies.truncate(k);
    coverage.truncate(k);
    let now = input.recorded_at_ms;
    let cache=json!({
            "basis":trace.basis.iter().map(|v|encode_vector(v)).collect::<Vec<_>>(),
            "mean":encode_vector(&mean),"energies":energies,"coverage":coverage,
            "labels":trace.picked.iter().map(|(id,_)|names[id].clone()).collect::<Vec<_>>(),
            "anchorTagIds":trace.picked.iter().map(|(id,_)|*id).collect::<Vec<_>>(),
            "totalEnergy":total,"kSem":k_sem,"kMax":k_max,"epaAlgorithm":"ags-global-star-t4-cached-reorth",
            "candidateMode":candidate_mode,"priorMode":prior_mode,"fullCoverage":full_coverage,
            "artifactSig":signature,"tagCount":ids.len(),"timestamp":now
        }).to_string();

    let computed = results.iter().filter(|r| r.status == "computed").count();
    let anchor = local.get("anchorGain").unwrap_or(local);
    let base = number(
        anchor,
        "base",
        number(local, "v9AnchorBase", 0.75, 0.0, 4.0),
        0.0,
        4.0,
    );
    let scale = number(
        anchor,
        "scale",
        number(local, "v9AnchorScale", 1.25, 0.0, 4.0),
        0.0,
        4.0,
    );
    let gamma = number(
        anchor,
        "gamma",
        number(local, "v9AnchorGamma", 1.0, 0.1, 8.0),
        0.1,
        8.0,
    );
    let low = number(
        anchor,
        "min",
        number(local, "v9AnchorMin", 0.5, 0.0, 4.0),
        0.0,
        4.0,
    );
    let high = number(
        anchor,
        "max",
        number(local, "v9AnchorMax", 2.0, 0.0, 8.0),
        0.0,
        8.0,
    );

    let anchor_gain = results
        .iter()
        .filter(|r| r.status == "computed")
        .map(|r| {
            (
                r.id,
                (base + scale * r.residual.powf(gamma)).clamp(low.min(high), low.max(high)),
            )
        })
        .collect();
    Ok(GeometryOutput {
        algorithm_version: ALGORITHM,
        signature,
        config_hash,
        tag_count: ids.len(),
        computed_count: computed,
        intrinsic: results,
        anchor_gain,
        mean,
        basis: trace.basis,
        coverage,
        epa_cache_json: cache,
    })
}
