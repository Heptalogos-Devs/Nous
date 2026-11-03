//! Request-local dynamic residual wormholes.
use super::*;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWormholeDiagnostics {
    pub targets_evaluated: usize,
    pub source_vectors_loaded: usize,
    pub source_batches: usize,
    pub dot_products: usize,
    pub cache_hits: usize,
    pub budget_exhausted: bool,
    pub load_ms: f64,
    pub score_ms: f64,
    pub selection_ms: f64,
}

/// Request-local only: never mutates graph assets or persists residuals.
/// A source at layer L is supported only by residuals from layers > L.
pub struct DynamicWormholeContext<'a> {
    pub(super) connection: &'a TagVectorPool,
    pub(super) pool: &'a mut TagVectorPool,
    pub(super) residuals: Vec<Vec<f32>>,
    pub(super) residual_norms: Vec<f64>,
    pub(super) known_scores: HashMap<(usize, i64), f64>,
    pub(super) seed_levels: HashMap<i64, usize>,
    pub(super) scores: HashMap<usize, Vec<f64>>,
    pub diagnostics: DynamicWormholeDiagnostics,
}

impl<'a> DynamicWormholeContext<'a> {
    pub fn try_new(
        connection: &'a TagVectorPool,
        pool: &'a mut TagVectorPool,
        residuals: Vec<Vec<f32>>,
        seed_levels: HashMap<i64, usize>,
    ) -> Result<Self, String> {
        if residuals.first().is_some_and(|first| {
            first.is_empty()
                || residuals.iter().any(|row| {
                    row.len() != first.len() || row.iter().any(|value| !value.is_finite())
                })
        }) || seed_levels.keys().any(|id| *id <= 0)
        {
            return Err("MEMO_RESIDUAL_CONTEXT_INVALID: residual dimensions, values or seed IDs are invalid".into());
        }
        let residual_norms = residuals
            .iter()
            .map(|r| r.iter().map(|&v| (v as f64).powi(2)).sum::<f64>().sqrt())
            .collect();
        Ok(Self {
            connection,
            pool,
            residuals,
            residual_norms,
            seed_levels,
            known_scores: HashMap::new(),
            scores: HashMap::new(),
            diagnostics: DynamicWormholeDiagnostics::default(),
        })
    }

    pub fn reuse_score(&mut self, layer: usize, id: i64, score: f64) {
        if score.is_finite() {
            self.known_scores
                .insert((layer, id), score.clamp(-1.0, 1.0));
        }
    }

    pub(super) fn prepare(
        &mut self,
        artifact: &NativeArtifact,
        active: &HashMap<(Option<usize>, usize), SpikeState>,
        config: &SenseConfig,
    ) -> std::result::Result<(), String> {
        if self.residuals.len() < 2 {
            return Ok(());
        }
        let mut targets = Vec::new();
        for spike in active.values().filter(|s| {
            s.energy >= config.firing_threshold
                && s.momentum >= 0.0
                && s.residual_level + 1 < self.residuals.len()
        }) {
            for cursor in
                artifact.row_offsets[spike.node_index]..artifact.row_offsets[spike.node_index + 1]
            {
                if positive(artifact.weights[cursor]) <= 0.0 {
                    break;
                }
                let target = artifact.targets[cursor];
                if !self.scores.contains_key(&target) {
                    targets.push(target);
                }
            }
        }
        // Deterministic request budget, independent of HashMap iteration order.
        targets.sort_unstable();
        targets.dedup();
        let remaining = config
            .dynamic_wormhole_max_targets
            .saturating_sub(self.scores.len());
        if targets.len() > remaining {
            self.diagnostics.budget_exhausted = true;
        }
        targets.truncate(remaining);
        if targets.is_empty() {
            return Ok(());
        }
        let dimension = self.residuals[0].len();
        let ids: Vec<i64> = targets.iter().map(|&i| artifact.node_ids[i]).collect();
        let began = Instant::now();
        let stats = ensure_tag_vectors_loaded(self.connection, self.pool, &ids, dimension)?;
        self.diagnostics.load_ms += began.elapsed().as_secs_f64() * 1000.0;
        self.diagnostics.source_vectors_loaded += stats.fetched_nodes;
        self.diagnostics.source_batches += stats.source_batches;
        let began = Instant::now();
        for target in targets {
            // Cache negative/missing results too: never retry a dirty SQL row per hop.
            let mut scores = vec![0.0; self.residuals.len()];
            if let Some(vector) = self.pool.get(&artifact.node_ids[target]) {
                let norm = vector
                    .iter()
                    .map(|&v| (v as f64).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if vector.len() == dimension && norm.is_finite() && norm > 1e-12 {
                    for (layer, residual) in self.residuals.iter().enumerate().skip(1) {
                        let residual_norm = self.residual_norms[layer];
                        if residual.len() != dimension {
                            continue;
                        }
                        if residual_norm.is_finite() && residual_norm > 1e-12 {
                            if let Some(&score) =
                                self.known_scores.get(&(layer, artifact.node_ids[target]))
                            {
                                scores[layer] = positive(score);
                                self.diagnostics.cache_hits += 1;
                            } else {
                                let dot = residual
                                    .iter()
                                    .zip(vector.iter())
                                    .map(|(&a, &b)| a as f64 * b as f64)
                                    .sum::<f64>();
                                scores[layer] = positive(dot / (norm * residual_norm)).min(1.0);
                                self.diagnostics.dot_products += 1;
                            }
                        }
                    }
                }
            }
            self.scores.insert(target, scores);
        }
        self.diagnostics.targets_evaluated = self.scores.len();
        self.diagnostics.score_ms += began.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub(super) fn resonance(&mut self, target: usize, source_level: usize) -> f64 {
        let Some(scores) = self.scores.get(&target) else {
            return 0.0;
        };
        self.diagnostics.cache_hits += 1;
        scores
            .iter()
            .skip(source_level.saturating_add(1))
            .copied()
            .fold(0.0, f64::max)
    }
}
