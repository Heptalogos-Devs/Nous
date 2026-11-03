//! Grounded method-stage planning.
use crate::assets::MemoAssets;
use crate::pipeline::PipelineOutput;
use crate::sensing::ObservationTransition;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub diary_name: String,
    pub k: usize,
    pub candidate_ids: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingInput {
    pub stages: Vec<Stage>,
    #[serde(default = "default_closure")]
    pub min_closure: f64,
}

#[derive(Clone)]
struct Module {
    id: i64,
    file_id: i64,
    source_ids: Vec<i64>,
    relevance: f64,
    tags: HashMap<i64, (usize, f64)>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub chunk_id: i64,
    pub score: f64,
    pub parents: Vec<i64>,
    pub relation: &'static str,
    pub route: Option<ObservationTransition>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StageOutput {
    pub diary_name: String,
    pub k: usize,
    pub results: Vec<Selection>,
}

pub fn default_closure() -> f64 {
    0.2
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let aa: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum();
    let bb: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum();
    let value = dot / (aa * bb).sqrt();
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn follows(left: &ObservationTransition, right: &ObservationTransition) -> bool {
    !right.immediate_return
        && left.hop + 1 == right.hop
        && left.target_id == right.source_id
        && right.previous_id == Some(left.source_id)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingOutput {
    pub schema: &'static str,
    pub artifact_sig: String,
    pub generation: String,
    pub stages: Vec<StageOutput>,
}
pub fn plan(
    assets: &MemoAssets,
    cached: &PipelineOutput,
    input: ThinkingInput,
) -> Result<ThinkingOutput, String> {
    cached.validate_for(assets)?;

    if input.stages.is_empty()
        || input.stages.len() > 32
        || !input.min_closure.is_finite()
        || !(0.0..=1.0).contains(&input.min_closure)
        || input.stages.iter().any(|s| {
            s.k > 32
                || s.candidate_ids.len() > 256
                || s.diary_name.trim().is_empty()
                || s.candidate_ids.iter().any(|id| *id <= 0)
        })
    {
        return Err("invalid or oversized method planning request".into());
    }

    let observation = &cached.observation;
    if observation.transitions.is_empty() || observation.transitions_truncated {
        return Err("Sense transitions unavailable or truncated".into());
    }
    let dimension = cached.original_query_vector.len();
    let artifact = &assets.graph;
    let mut output = Vec::new();
    let mut selected: Vec<Selection> = Vec::new();
    let mut used_files = HashSet::new();
    let mut used_edges = HashSet::new();
    let maximum_flow = observation
        .transitions
        .iter()
        .map(|edge| edge.flow)
        .fold(0.0, f64::max);

    for stage in input.stages {
        let mut modules = Vec::new();
        let mut seen = HashSet::new();
        for id in stage.candidate_ids {
            if id <= 0 || !seen.insert(id) {
                continue;
            }
            let Some(data) = assets.candidates.get(&id) else {
                continue;
            };
            let Some(file) = assets.files.get(&data.file_id) else {
                continue;
            };
            if file.diary_name != stage.diary_name
                || data.vector.len() != dimension
                || data.vector.iter().any(|value| !value.is_finite())
            {
                continue;
            }
            let mut rows = file.tags.clone();
            rows.sort_by_key(|row| (row.position, row.tag_id));
            let mut tags = HashMap::new();
            for (position, row) in rows.iter().take(128).enumerate() {
                if let Some(vector) = assets.tag_vectors.get(&row.tag_id)
                    && vector.len() == dimension
                    && vector.iter().all(|v| v.is_finite())
                {
                    tags.insert(row.tag_id, (position, cosine(&data.vector, vector)));
                }
            }
            modules.push(Module {
                id,
                file_id: data.file_id,
                source_ids: data.source_ids.clone(),
                tags,
                relevance: cosine(&data.vector, &cached.original_query_vector),
            });
        }
        let mut results = Vec::new();
        for _ in 0..stage.k {
            let mut offers = Vec::new();
            for module in &modules {
                if used_files.contains(&module.file_id) {
                    continue;
                }
                let mut best_route = None;
                let mut best_parents = Vec::new();
                let mut best_route_score = 0.0;
                for edge in &observation.transitions {
                    if edge.immediate_return {
                        continue;
                    }
                    let (Some((a_pos, a_closure)), Some((b_pos, b_closure))) = (
                        module.tags.get(&edge.source_id),
                        module.tags.get(&edge.target_id),
                    ) else {
                        continue;
                    };
                    if *a_pos >= *b_pos
                        || *a_closure < input.min_closure
                        || *b_closure < input.min_closure
                    {
                        continue;
                    }
                    let parents: Vec<i64> = selected
                        .iter()
                        .filter(|s| {
                            s.route
                                .as_ref()
                                .is_some_and(|previous| follows(previous, edge))
                        })
                        .map(|s| s.chunk_id)
                        .collect();
                    if edge.hop != 1 && parents.is_empty() {
                        continue;
                    }
                    let edge_key = (edge.hop, edge.previous_id, edge.source_id, edge.target_id);
                    let novelty = if used_edges.contains(&edge_key) {
                        0.2
                    } else {
                        1.0
                    };
                    let independent = artifact.independent_fraction_excluding(
                        edge.source_id,
                        edge.target_id,
                        &module.source_ids,
                    );
                    let score = (edge.flow / maximum_flow.max(1e-12))
                        * (a_closure * b_closure).sqrt()
                        * independent
                        * novelty;
                    let score = score * if parents.is_empty() { 0.8 } else { 1.0 };
                    if score > best_route_score {
                        best_route_score = score;
                        best_route = Some(edge.clone());
                        best_parents = parents;
                    }
                }
                let relation = match &best_route {
                    Some(edge) if edge.wormhole => "analogy",
                    Some(_) if !best_parents.is_empty() => "continuation",
                    Some(_) => "root",
                    None => "supplement",
                };
                offers.push((
                    module.file_id,
                    Selection {
                        chunk_id: module.id,
                        score: 0.35 * module.relevance + 0.65 * best_route_score,
                        parents: best_parents,
                        relation,
                        route: best_route,
                    },
                ));
            }
            offers.sort_by(|a, b| {
                b.1.route
                    .is_some()
                    .cmp(&a.1.route.is_some())
                    .then_with(|| b.1.score.total_cmp(&a.1.score))
                    .then_with(|| a.1.chunk_id.cmp(&b.1.chunk_id))
            });
            let Some((file_id, choice)) = offers.into_iter().next() else {
                break;
            };
            used_files.insert(file_id);
            if let Some(edge) = &choice.route {
                used_edges.insert((edge.hop, edge.previous_id, edge.source_id, edge.target_id));
            }
            selected.push(choice.clone());
            results.push(choice);
        }
        output.push(StageOutput {
            diary_name: stage.diary_name,
            k: stage.k,
            results,
        });
    }

    if !selected.iter().any(|selection| selection.route.is_some()) {
        return Err("no grounded method route in the accepted observation".into());
    }
    Ok(ThinkingOutput {
        schema: "vcp-river-thinking-plan-v1",
        artifact_sig: assets.signature.clone(),
        generation: assets.generation.clone(),
        stages: output,
    })
}
