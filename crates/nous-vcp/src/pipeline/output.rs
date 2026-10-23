//! Complete pipeline observation and generation validation.
// Adapted from VCPToolBox d1b40c863273cbbba082fb7657671424768c4a53.
// CC BY-NC-SA 4.0; see LICENSE-VCP and UPSTREAM.md.
use super::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PipelineDiagnostics {
    pub(super) backend: String,
    pub(super) runtime_ownership: String,
    pub(super) epa_ms: f64,
    pub(super) pyramid_ms: f64,
    pub(super) gating_ms: f64,
    pub(super) sensing_ms: f64,
    pub(super) fusion_ms: f64,
    pub(super) dual_field_solve_ms: f64,
    pub(super) dual_field_project_ms: f64,
    pub(super) dual_field_domain_ms: f64,
    pub(super) total_ms: f64,
    pub(super) seed_nodes: usize,
    pub(super) observation_cached: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) observation_cache_error: Option<String>,
    pub(super) dual_field: DualFieldDiagnostics,
    pub(super) fusion: FusionDiagnostics,
    pub(super) sensing: SenseDiagnostics,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PipelineMetadataOutput {
    pub(super) schema: String,
    pub(super) algorithm_version: String,
    pub(super) artifact_sig: String,
    pub(super) query_id: Option<String>,
    pub(super) epa: EpaOutput,
    pub(super) pyramid: PyramidOutput,
    pub(super) matched_tags: Vec<String>,
    pub(super) core_tags_matched: Vec<String>,
    pub(super) effective_tag_boost: f64,
    pub(super) diagnostics: PipelineDiagnostics,
}

pub struct PipelineOutput {
    pub generation: String,
    pub artifact_sig: String,
    pub original_query_vector: Vec<f32>,
    pub enhanced_vector: Vec<f32>,
    pub local_vector: Vec<f32>,
    pub transfer_vector: Vec<f32>,
    pub local_field: Vec<(i64, f64)>,
    pub transfer_field: Vec<(i64, f64)>,
    pub local_domain_ids: Vec<i64>,
    pub transfer_domain_ids: Vec<i64>,
    pub observation: SenseOutput,
    pub tag_vectors: TagVectorPool,
    pub metadata: serde_json::Value,
}
impl PipelineOutput {
    /// Validate the generation and request-local numerical state consumed by readout.
    pub fn validate_for(&self, assets: &MemoAssets) -> Result<(), String> {
        if self.generation != assets.generation
            || self.artifact_sig != assets.signature
            || self.observation.artifact_sig != assets.signature
        {
            return Err("MEMO_OBSERVATION_STALE: asset generation or signature changed".into());
        }
        let vectors = [
            &self.original_query_vector,
            &self.enhanced_vector,
            &self.local_vector,
            &self.transfer_vector,
        ];
        if vectors.iter().any(|vector| {
            vector.len() != assets.dimension || vector.iter().any(|value| !value.is_finite())
        }) || self.tag_vectors.iter().any(|(id, vector)| {
            *id <= 0
                || vector.len() != assets.dimension
                || vector.iter().any(|value| !value.is_finite())
        }) {
            return Err(
                "MEMO_QUERY_VECTOR_INVALID: dimension or finite-value validation failed".into(),
            );
        }
        let valid_id = |id: i64| assets.graph.node_index.contains_key(&id);
        if self
            .local_field
            .iter()
            .chain(&self.transfer_field)
            .chain(&self.observation.source_field)
            .any(|(id, value)| !valid_id(*id) || !value.is_finite())
            || self
                .local_domain_ids
                .iter()
                .chain(&self.transfer_domain_ids)
                .any(|id| !valid_id(*id))
            || self.observation.nodes.iter().any(|node| {
                !valid_id(node.id)
                    || !node.energy.is_finite()
                    || !node.normalized_energy.is_finite()
            })
            || self.observation.edges.iter().any(|edge| {
                !valid_id(edge.source_id)
                    || !valid_id(edge.target_id)
                    || [
                        edge.flow,
                        edge.max_flow,
                        edge.normalized_flow,
                        edge.conductance,
                    ]
                    .iter()
                    .any(|value| !value.is_finite())
            })
            || self.observation.transitions.iter().any(|edge| {
                !valid_id(edge.source_id)
                    || !valid_id(edge.target_id)
                    || edge.previous_id.is_some_and(|id| !valid_id(id))
                    || !edge.flow.is_finite()
            })
        {
            return Err(
                "MEMO_QUERY_STATE_INVALID: observation IDs or finite values are invalid".into(),
            );
        }
        Ok(())
    }
}
