//! Readout configuration, curve state and result contracts.
use super::*;

pub(super) fn default_top_k() -> usize {
    10
}

pub(super) fn default_dimension() -> usize {
    3072
}

pub(super) fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NativeInput {
    #[serde(default = "default_dimension")]
    pub(super) dimension: usize,
    #[serde(default = "default_top_k")]
    pub(super) top_k: usize,
    #[serde(default)]
    pub(super) include_trace: bool,
    pub(super) query: QueryInput,
    #[serde(default)]
    pub(super) denoised_vector: Vec<f32>,
    #[serde(default)]
    pub(super) local_vector: Vec<f32>,
    #[serde(default)]
    pub(super) transfer_vector: Vec<f32>,
    #[serde(default)]
    pub(super) candidates: Vec<CandidateInput>,
    pub(super) query_state: QueryStateInput,
    #[serde(default)]
    pub(super) allowed_file_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub(super) config: NativeConfig,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueryInput {
    #[serde(default, rename = "text")]
    pub(super) _text: String,
    #[serde(default)]
    pub(super) vector: Vec<f32>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CandidateInput {
    pub id: i64,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub hybrid_score: f64,
    #[serde(default)]
    pub vector_score: f64,
    #[serde(default)]
    pub bm25_score: f64,
    #[serde(default)]
    pub time_score: f64,
    #[serde(default)]
    pub anchor_score: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QueryStateInput {
    #[serde(default)]
    pub(super) query_id: Option<String>,
    #[serde(default)]
    pub(super) source_field: Vec<(i64, f64)>,
    #[serde(default)]
    pub(super) local_field: Vec<(i64, f64)>,
    #[serde(default)]
    pub(super) transfer_field: Vec<(i64, f64)>,
    #[serde(default)]
    pub(super) local_domain_ids: Vec<i64>,
    #[serde(default)]
    pub(super) transfer_domain_ids: Vec<i64>,
    #[serde(default)]
    pub(super) river_nodes: Vec<RiverNode>,
    #[serde(default)]
    pub(super) river_edges: Vec<RiverEdge>,
    #[serde(default)]
    pub(super) field_provenance: Vec<SourceProvenance>,
    #[serde(default = "default_true")]
    pub(super) complete_observation: bool,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct RiverNode {
    pub(super) id: i64,
    #[serde(default)]
    pub(super) energy: f64,
    #[serde(default)]
    pub(super) normalized_energy: f64,
    #[serde(default)]
    pub(super) hop: i64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct RiverEdge {
    pub(super) source_id: i64,
    pub(super) target_id: i64,
    #[serde(default)]
    pub(super) flow: f64,
    #[serde(default)]
    pub(super) normalized_flow: f64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceProvenance {
    pub(super) id: i64,
    #[serde(default)]
    pub(super) hop: i64,
    #[serde(default)]
    pub(super) source_type: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct NativeConfig {
    pub query_k: usize,
    pub denoised_k: usize,
    pub local_field_k: usize,
    pub transfer_field_k: usize,
    pub bm25_k: usize,
    /// Sparse share of the base score when authorized lexical hits are present.
    pub lexical_weight: f64,
    pub anchor_k: usize,
    pub max_union_candidates: usize,
    pub local_weight: f64,
    pub transfer_weight: f64,
    pub direction_floor: f64,
    pub closure_floor: f64,
    pub semantic_node_threshold: f64,
    pub relative_distance_temperature: f64,
    pub reverse_direction_credit: f64,
    pub minimum_river_edge_flow: f64,
    pub maximum_river_edges: usize,
    pub node_only_reliability_cap: f64,
    pub kappa_edge: f64,
    pub kappa_ratio: f64,
    pub omega_epsilon: f64,
    pub collapsed_threshold: f64,
    pub sparse_threshold: f64,
    pub semantic_anchor_threshold: f64,
    pub semantic_anchor_discount: f64,
    pub specificity_floor: f64,
    pub rarity_floor: f64,
    pub reliability_seed_saturation: f64,
    pub fallback_reliability_cap: f64,
    pub pure_query_weight: f64,
    pub pure_local_weight: f64,
    pub pure_transfer_weight: f64,
    pub topology_bonus_cap: f64,
    pub topology_path_saturation: f64,
    pub conditional_bandwidth: f64,
    pub conditional_closure_bandwidth: f64,
    pub conditional_direct_bandwidth: f64,
    pub minimum_peers: usize,
    pub minimum_effective_peers: f64,
    pub innovation_confidence_z: f64,
    pub innovation_scale: f64,
    pub omega_gamma: f64,
    pub struct_role_min_omega: f64,
    pub anchor_bonus_cap: f64,
    pub anchor_activation_z: f64,
    pub anchor_activation_floor: f64,
    pub anchor_saturation: f64,
    pub anchor_frontier_contrast: f64,
    pub anchor_frontier_abs_floor: f64,
}

impl Default for NativeConfig {
    fn default() -> Self {
        Self {
            query_k: 100,
            denoised_k: 100,
            local_field_k: 100,
            transfer_field_k: 100,
            bm25_k: 50,
            lexical_weight: 0.6,
            anchor_k: 50,
            max_union_candidates: 300,
            local_weight: 0.6,
            transfer_weight: 0.4,
            direction_floor: 0.05,
            closure_floor: 0.0,
            semantic_node_threshold: 0.48,
            relative_distance_temperature: 0.35,
            reverse_direction_credit: 0.25,
            minimum_river_edge_flow: 0.015,
            maximum_river_edges: 96,
            node_only_reliability_cap: 0.2,
            kappa_edge: 0.5,
            kappa_ratio: 0.3,
            omega_epsilon: 0.02,
            collapsed_threshold: 0.12,
            sparse_threshold: 0.45,
            semantic_anchor_threshold: 0.8,
            semantic_anchor_discount: 0.7,
            specificity_floor: 0.35,
            rarity_floor: 0.15,
            reliability_seed_saturation: 2.0,
            fallback_reliability_cap: 0.5,
            pure_query_weight: 0.25,
            pure_local_weight: 0.2,
            pure_transfer_weight: 0.15,
            topology_bonus_cap: 0.08,
            topology_path_saturation: 0.15,
            conditional_bandwidth: 0.04,
            conditional_closure_bandwidth: 0.1,
            conditional_direct_bandwidth: 0.12,
            minimum_peers: 3,
            minimum_effective_peers: 2.5,
            innovation_confidence_z: 1.0,
            innovation_scale: 0.5,
            omega_gamma: 1.0,
            struct_role_min_omega: 0.12,
            anchor_bonus_cap: 0.1,
            anchor_activation_z: 2.0,
            anchor_activation_floor: 0.05,
            anchor_saturation: 0.2,
            anchor_frontier_contrast: 2.0,
            anchor_frontier_abs_floor: 0.1,
        }
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TagData {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) position: i64,
    /// 同一 Tag 在所有候选曲线间共享同一份解码向量，只做引用计数克隆。
    pub(super) vector: Arc<[f32]>,
    pub(super) chunk_cosine: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Curve {
    pub(super) id: i64,
    pub(super) file_id: i64,
    pub(super) source_ids: Vec<i64>,
    pub(super) tags: Vec<TagData>,
    pub(super) chunk_vector: Arc<[f32]>,
    pub(super) query_score: f64,
    pub(super) denoised_score: f64,
    pub(super) local_score: f64,
    pub(super) transfer_score: f64,
    pub(super) bm25_score: f64,
    pub(super) lexical_score: f64,
    pub(super) time_score: f64,
    pub(super) anchor_score: f64,
    /// Explicit candidate input, separate from local-domain selection contacts.
    pub(super) direct_anchor_score: f64,
    pub(super) union_score: f64,
    pub(super) union_rank: usize,
    pub(super) sources: Vec<String>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CurveLoadOutput {
    /// Candidate curves without Tags; `attach_tags` adds them after selection.
    pub(super) curves: Vec<Curve>,
    pub(super) relations: FileTagRows,
    pub(super) chunk_source_batches: usize,
    pub(super) file_tag_source_batches: usize,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FieldWorkspace {
    pub(super) local: HashMap<i64, f64>,
    pub(super) transfer: HashMap<i64, f64>,
    pub(super) local_domain: HashSet<i64>,
    pub(super) transfer_domain: HashSet<i64>,
    pub(super) source_ids: HashSet<i64>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeometryOutput {
    pub path_quality: f64,
    pub path_core: f64,
    pub tag_closure: f64,
    pub support_coverage: f64,
    pub segment_count: usize,
    pub supported_segments: usize,
    pub transfer_segments: usize,
    pub mean_direction: f64,
    pub mean_continuity: f64,
    pub mean_local_potential: f64,
    pub mean_transfer_potential: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyOutput {
    pub score: f64,
    pub reliability: f64,
    pub reliability_mode: String,
    pub node_alignment_score: f64,
    pub edge_graph_score: f64,
    pub node_graph_score: f64,
    pub relative_distance_score: f64,
    pub direction_score: f64,
    pub edge_topology_score: f64,
    pub motif_score: f64,
    pub matched_node_coverage: f64,
    pub matched_edge_coverage: f64,
    pub mean_closure: f64,
    pub matched_nodes: usize,
    pub matched_edges: usize,
    pub query_nodes: usize,
    pub query_edges: usize,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Alignment {
    pub(super) candidate_index: usize,
    pub(super) candidate_position: i64,
    pub(super) quality: f64,
    pub(super) closure: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservableOutput {
    pub direct: f64,
    pub structural: f64,
    pub thematic: f64,
    pub closure: f64,
    pub query_chunk_score: f64,
    pub semantic_boundary_score: f64,
    pub local_coverage: f64,
    pub transfer_coverage: f64,
    pub local_potential: f64,
    pub transfer_potential: f64,
    pub tail_only_ratio: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AnchorOutput {
    pub(super) score: f64,
    pub(super) reliability: f64,
    pub(super) strength: f64,
    pub(super) contacted_seeds: usize,
    pub(super) exact_contacts: usize,
    pub(super) semantic_contacts: usize,
    pub(super) mean_closure: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScoredWork {
    pub(super) curve: Curve,
    pub(super) geometry: GeometryOutput,
    pub(super) topology: TopologyOutput,
    pub(super) observables: ObservableOutput,
    pub(super) pure_score: f64,
    pub(super) semantic_score: f64,
    pub(super) lexical_base: f64,
    pub(super) graph_score: f64,
    pub(super) direct_evidence: f64,
    pub(super) role: String,
    pub(super) anchor: AnchorOutput,
    pub(super) v2_bonus: f64,
    pub(super) gated_bonus: f64,
    pub(super) anchor_bonus: f64,
    pub(super) final_score: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryMorphology {
    pub atomic_weight: f64,
    pub propositional_weight: f64,
    pub narrative_weight: f64,
    pub confidence: f64,
    pub effective_depth: f64,
    pub depth_variance: f64,
    pub energy_concentration: f64,
    pub shallow_energy_ratio: f64,
    pub forward_flow_ratio: f64,
    pub same_level_flow_ratio: f64,
    pub chainness: f64,
    pub branching: f64,
    pub merging: f64,
    pub growth_persistence: f64,
    pub dominant_mode: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmegaOutput {
    pub omega: f64,
    pub omega_edge: f64,
    pub omega_emerge: f64,
    pub omega_flow: f64,
    pub regime: String,
    pub active_edges: usize,
    pub seed_nodes: usize,
    pub reached_nodes: usize,
    pub emergent_nodes: usize,
    pub complete_observation: bool,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeResultItem {
    pub id: i64,
    pub chunk_id: i64,
    pub rank: usize,
    pub score: f64,
    pub base_score: f64,
    pub topology_bonus: f64,
    pub anchor_bonus: f64,
    pub role: String,
    pub omega: f64,
    pub river_regime: String,
    pub matched_tags: Vec<String>,
    pub core_tags_matched: Vec<String>,
    pub candidate_sources: Vec<String>,
    pub original_score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topology_v3: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_topology: Option<TopologyOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geometry: Option<GeometryOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observables: Option<ObservableOutput>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeDiagnostics {
    pub backend: String,
    pub offered_candidates: usize,
    pub projected_candidates: usize,
    pub selected_candidates: usize,
    pub ranked_candidates: usize,
    pub returned_candidates: usize,
    pub rayon_threads: usize,
    pub artifact_nodes: usize,
    pub artifact_edges: usize,
    pub load_ms: f64,
    pub compute_ms: f64,
    pub total_ms: f64,
    pub chunk_source_batches: usize,
    pub file_tag_source_batches: usize,
    pub query_tag_source_batches: usize,
    pub candidate_tag_vector_source_batches: usize,
    pub candidate_tag_vector_fetched_nodes: usize,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeOutput {
    pub schema: String,
    pub algorithm_version: String,
    pub artifact_sig: String,
    pub query_id: Option<String>,
    pub omega: OmegaOutput,
    pub query_mode: String,
    pub query_morphology: QueryMorphology,
    pub results: Vec<NativeResultItem>,
    pub diagnostics: NativeDiagnostics,
}
