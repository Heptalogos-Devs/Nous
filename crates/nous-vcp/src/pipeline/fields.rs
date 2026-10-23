//! Dual scalar fields, bounded basin projection and support domains.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DualFieldDiagnostics {
    pub(super) retained_local_field_nodes: usize,
    pub(super) retained_transfer_field_nodes: usize,
    pub(super) local_domain_nodes: usize,
    pub(super) transfer_domain_nodes: usize,
    pub(super) projected_nodes: usize,
    pub(super) selected_projection_nodes: usize,
    pub(super) source_vectors_loaded: usize,
    pub(super) source_requested_nodes: usize,
    pub(super) source_batches: usize,
    pub(super) growth: crate::projection::GrowthDiagnostics,
    pub(super) local_support: ProjectionSupportDiagnostics,
    pub(super) transfer_support: ProjectionSupportDiagnostics,
    pub(super) local_selected_mass_ratio: f64,
    pub(super) transfer_selected_mass_ratio: f64,
    pub(super) local_projected_mass_ratio: f64,
    pub(super) transfer_projected_mass_ratio: f64,
    pub(super) solve_ms: f64,
    pub(super) domain_ms: f64,
    pub(super) project_ms: f64,
    pub(super) iterations: usize,
    pub(super) local_converged: bool,
    pub(super) transfer_converged: bool,
    pub(super) local_residual: f64,
    pub(super) transfer_residual: f64,
    pub(super) local_mass: f64,
    pub(super) transfer_mass: f64,
}

pub(super) struct DualFieldOutput {
    pub(super) local_field: Vec<(i64, f64)>,
    pub(super) transfer_field: Vec<(i64, f64)>,
    pub(super) local_domain_ids: Vec<i64>,
    pub(super) transfer_domain_ids: Vec<i64>,
    pub(super) local_vector: Vec<f32>,
    pub(super) transfer_vector: Vec<f32>,
    pub(super) diagnostics: DualFieldDiagnostics,
}

pub(super) fn apply_transport(
    artifact: &crate::graph::NativeArtifact,
    input: &[f64],
    output: &mut [f64],
) {
    output.fill(0.0);
    for source in 0..artifact.node_ids.len() {
        let mass = input[source];
        if mass == 0.0 {
            continue;
        }
        for cursor in artifact.row_offsets[source]..artifact.row_offsets[source + 1] {
            output[artifact.targets[cursor]] += mass * positive(artifact.weights[cursor]);
        }
    }
}

/// `apply_transport` for both fields in one pass over the CSR. Every output
/// position receives the same additions in the same order as two separate
/// passes, and each field keeps its own zero-mass skip.
pub(super) fn apply_transport_pair(
    artifact: &crate::graph::NativeArtifact,
    local: &[f64],
    transfer: &[f64],
    local_output: &mut [f64],
    transfer_output: &mut [f64],
) {
    local_output.fill(0.0);
    transfer_output.fill(0.0);
    for source in 0..artifact.node_ids.len() {
        let local_mass = local[source];
        let transfer_mass = transfer[source];
        let local_active = local_mass != 0.0;
        let transfer_active = transfer_mass != 0.0;
        if !local_active && !transfer_active {
            continue;
        }
        for cursor in artifact.row_offsets[source]..artifact.row_offsets[source + 1] {
            let weight = positive(artifact.weights[cursor]);
            let target = artifact.targets[cursor];
            if local_active {
                local_output[target] += local_mass * weight;
            }
            if transfer_active {
                transfer_output[target] += transfer_mass * weight;
            }
        }
    }
}

/// `next = (1 - alpha) * source + alpha * propagated` and the L1 change from
/// `current`, in one pass with the same arithmetic and index order as a
/// separate update followed by an L1 sum of non-negative terms.
pub(super) fn relax_field(
    source: &[f64],
    propagated: &[f64],
    alpha: f64,
    current: &[f64],
    next: &mut [f64],
) -> f64 {
    let mut residual = 0.0;
    for index in 0..source.len() {
        next[index] = (1.0 - alpha) * source[index] + alpha * propagated[index];
        residual += (next[index] - current[index]).abs();
    }
    residual
}

pub(super) fn field_mass(field: &[f64]) -> f64 {
    field.iter().map(|value| positive(*value)).sum()
}

pub(super) fn effective_domain(
    artifact: &crate::graph::NativeArtifact,
    field: &[f64],
    ratio: f64,
) -> Vec<i64> {
    let total = field_mass(field);
    if total <= 0.0 {
        return Vec::new();
    }
    let mut ranked: Vec<(i64, f64)> = artifact
        .node_ids
        .iter()
        .copied()
        .zip(field.iter().copied())
        .filter(|(_, mass)| *mass > 0.0)
        .collect();
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut retained = Vec::new();
    let mut mass = 0.0;
    for (id, value) in ranked {
        retained.push(id);
        mass += value;
        if mass / total >= ratio.clamp(0.01, 1.0) {
            break;
        }
    }
    retained
}

/// Legacy full-support projection retained only as a regression reference.

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectionSupportDiagnostics {
    pub(super) core_nodes: usize,
    pub(super) selected_nodes: usize,
    pub(super) outside_domain_nodes: usize,
    pub(super) missing_vector_nodes: usize,
    pub(super) stop_reason: String,
}

pub(super) fn project_basin_fields(
    connection: &MemoAssets,
    artifact: &crate::graph::NativeArtifact,
    fields: [&[f64]; 2],
    domains: [&[i64]; 2],
    dimension: usize,
    limit: usize,
    river_ids: &[i64],
    basins: &[crate::projection::Basin],
    pool: &mut TagVectorPool,
    diagnostics: &mut DualFieldDiagnostics,
) -> std::result::Result<(Vec<f32>, Vec<f32>), String> {
    let supports = crate::projection::grow(artifact, fields, domains, river_ids, basins, limit)?;
    let selected = supports.ids;
    let support_diagnostics = |f: usize| {
        let domain: HashSet<i64> = domains[f].iter().copied().collect();
        ProjectionSupportDiagnostics {
            core_nodes: supports.diagnostics.core_nodes,
            selected_nodes: selected[f].len(),
            outside_domain_nodes: selected[f].iter().filter(|id| !domain.contains(id)).count(),
            stop_reason: supports.diagnostics.stop_reason.clone(),
            ..ProjectionSupportDiagnostics::default()
        }
    };
    diagnostics.local_support = support_diagnostics(0);
    diagnostics.transfer_support = support_diagnostics(1);
    diagnostics.growth = supports.diagnostics;
    let mut union: Vec<i64> = selected.iter().flatten().copied().collect();
    union.sort_unstable();
    union.dedup();
    diagnostics.selected_projection_nodes = union.len();
    let stats = ensure_tag_vectors_loaded(&connection.tag_vectors, pool, &union, dimension)?;
    diagnostics.source_vectors_loaded = stats.fetched_nodes;
    diagnostics.source_requested_nodes = stats.requested_nodes;
    diagnostics.source_batches = stats.source_batches;
    diagnostics.projected_nodes = union.iter().filter(|id| pool.contains_key(id)).count();
    diagnostics.local_support.missing_vector_nodes = selected[0]
        .iter()
        .filter(|id| !pool.contains_key(id))
        .count();
    diagnostics.transfer_support.missing_vector_nodes = selected[1]
        .iter()
        .filter(|id| !pool.contains_key(id))
        .count();
    let mut outputs = [vec![0.0f64; dimension], vec![0.0f64; dimension]];
    let mut selected_ratios = [0.0; 2];
    let mut projected_ratios = [0.0; 2];
    for field in 0..2 {
        let total_mass = field_mass(fields[field]);
        let mut selected_mass = 0.0;
        let mut projected_mass = 0.0;
        for id in &selected[field] {
            let Some(&index) = artifact.node_index.get(id) else {
                continue;
            };
            let mass = positive(fields[field][index]);
            selected_mass += mass;
            let Some(vector) = pool.get(id) else { continue };
            projected_mass += mass;
            for (out, value) in outputs[field].iter_mut().zip(vector.iter()) {
                *out += *value as f64 * mass;
            }
        }
        if projected_mass > 0.0 {
            for out in &mut outputs[field] {
                *out /= projected_mass;
            }
            normalize(&mut outputs[field]);
        }
        if total_mass > 0.0 {
            selected_ratios[field] = selected_mass / total_mass;
            projected_ratios[field] = projected_mass / total_mass;
        }
    }
    [
        diagnostics.local_selected_mass_ratio,
        diagnostics.transfer_selected_mass_ratio,
    ] = selected_ratios;
    [
        diagnostics.local_projected_mass_ratio,
        diagnostics.transfer_projected_mass_ratio,
    ] = projected_ratios;
    let [local, transfer] = outputs;
    Ok((
        local.into_iter().map(|v| v as f32).collect(),
        transfer.into_iter().map(|v| v as f32).collect(),
    ))
}

pub(super) fn solve_dual_fields_with_pool(
    artifact: &crate::graph::NativeArtifact,
    connection: &MemoAssets,
    source_entries: &[(i64, f64)],
    dimension: usize,
    config: &PipelineConfig,
    pool: &mut TagVectorPool,
    river_ids: &[i64],
    basins: &[crate::projection::Basin],
) -> std::result::Result<DualFieldOutput, String> {
    let solve_started = Instant::now();
    let mut source = vec![0.0f64; artifact.node_ids.len()];
    for (id, mass) in source_entries {
        if let Some(index) = artifact.node_index.get(id) {
            source[*index] += positive(*mass);
        }
    }
    let source_mass = field_mass(&source);
    if source_mass <= 0.0 {
        return Ok(DualFieldOutput {
            local_field: Vec::new(),
            transfer_field: Vec::new(),
            local_domain_ids: Vec::new(),
            transfer_domain_ids: Vec::new(),
            local_vector: vec![0.0; dimension],
            transfer_vector: vec![0.0; dimension],
            diagnostics: DualFieldDiagnostics {
                iterations: 0,
                local_converged: true,
                transfer_converged: true,
                local_residual: 0.0,
                transfer_residual: 0.0,
                local_mass: 0.0,
                transfer_mass: 0.0,
                ..DualFieldDiagnostics::default()
            },
        });
    }
    for value in &mut source {
        *value /= source_mass;
    }

    let local_alpha = config.local_alpha.clamp(0.0, 0.999999);
    let transfer_alpha = config.transfer_alpha.clamp(0.0, 0.999999);
    let mut local = source.clone();
    let mut transfer = source.clone();
    let mut next_local = vec![0.0; source.len()];
    let mut next_transfer = vec![0.0; source.len()];
    let mut propagated_local = vec![0.0; source.len()];
    let mut propagated_transfer = vec![0.0; source.len()];
    let mut local_converged = false;
    let mut transfer_converged = false;
    let mut local_residual = f64::INFINITY;
    let mut transfer_residual = f64::INFINITY;
    let mut iterations = 0;

    for iteration in 1..=config.field_max_iterations.max(1) {
        iterations = iteration;
        // The fields are independent; while both move, one CSR pass serves both.
        if !local_converged && !transfer_converged {
            apply_transport_pair(
                artifact,
                &local,
                &transfer,
                &mut propagated_local,
                &mut propagated_transfer,
            );
        } else if !local_converged {
            apply_transport(artifact, &local, &mut propagated_local);
        } else if !transfer_converged {
            apply_transport(artifact, &transfer, &mut propagated_transfer);
        }
        if !local_converged {
            local_residual = relax_field(
                &source,
                &propagated_local,
                local_alpha,
                &local,
                &mut next_local,
            );
            local_converged = local_residual <= config.local_tolerance.max(1e-15);
            std::mem::swap(&mut local, &mut next_local);
        }
        if !transfer_converged {
            transfer_residual = relax_field(
                &source,
                &propagated_transfer,
                transfer_alpha,
                &transfer,
                &mut next_transfer,
            );
            transfer_converged = transfer_residual <= config.transfer_tolerance.max(1e-15);
            std::mem::swap(&mut transfer, &mut next_transfer);
        }
        if local_converged && transfer_converged {
            break;
        }
    }

    let solve_ms = solve_started.elapsed().as_secs_f64() * 1000.0;
    let domain_started = Instant::now();
    let local_domain_ids = effective_domain(artifact, &local, config.local_mass_ratio);
    let transfer_domain_ids = effective_domain(artifact, &transfer, config.transfer_mass_ratio);
    // Scalar retention is independent of projection K. Keep meaningful tails for
    // path potentials/tail penalties, and always preserve support/river nodes.
    let mut retained: Vec<i64> = local_domain_ids
        .iter()
        .chain(&transfer_domain_ids)
        .chain(river_ids)
        .copied()
        .collect();
    retained.sort_unstable();
    retained.dedup();
    let required: HashSet<i64> = retained.into_iter().collect();
    let scalar = |field: &[f64]| -> Vec<(i64, f64)> {
        let maximum = field.iter().copied().fold(0.0f64, f64::max);
        let cutoff = maximum * 1e-12;
        artifact
            .node_ids
            .iter()
            .copied()
            .zip(field.iter().copied())
            .filter(|(id, mass)| {
                mass.is_finite() && *mass > 0.0 && (*mass >= cutoff || required.contains(id))
            })
            .collect()
    };
    let local_field = scalar(&local);
    let transfer_field = scalar(&transfer);
    let local_mass = field_mass(&local);
    let transfer_mass = field_mass(&transfer);
    let mut projection = DualFieldDiagnostics {
        solve_ms,
        retained_local_field_nodes: local_field.len(),
        retained_transfer_field_nodes: transfer_field.len(),
        local_domain_nodes: local_domain_ids.len(),
        transfer_domain_nodes: transfer_domain_ids.len(),
        domain_ms: domain_started.elapsed().as_secs_f64() * 1000.0,
        ..DualFieldDiagnostics::default()
    };
    let project_started = Instant::now();
    let (local_vector, transfer_vector) = project_basin_fields(
        connection,
        artifact,
        [&local, &transfer],
        [&local_domain_ids, &transfer_domain_ids],
        dimension,
        config.max_field_projection_nodes,
        river_ids,
        basins,
        pool,
        &mut projection,
    )?;
    projection.project_ms = project_started.elapsed().as_secs_f64() * 1000.0;

    Ok(DualFieldOutput {
        local_field,
        transfer_field,
        local_domain_ids,
        transfer_domain_ids,
        local_vector,
        transfer_vector,
        diagnostics: DualFieldDiagnostics {
            iterations,
            local_converged,
            transfer_converged,
            local_residual,
            transfer_residual,
            local_mass,
            transfer_mass,
            ..projection
        },
    })
}
