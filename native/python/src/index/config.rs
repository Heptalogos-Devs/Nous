//! Typed configuration and the JSON metadata schema at the Python boundary.
use super::*;
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StoreOptions {
    pub threads: usize,
}
impl Default for StoreOptions {
    fn default() -> Self {
        Self { threads: 2 }
    }
}
#[derive(Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct TagMetadata {
    pub key: String,
    pub label: String,
}
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Assets {
    pub space: String,
    pub tags: Vec<TagMetadata>,
    pub geometry_config: Value,
    pub artifact_config: Value,
}
impl Default for Assets {
    fn default() -> Self {
        Self {
            space: "default".into(),
            tags: vec![],
            geometry_config: json!({}),
            artifact_config: json!({}),
        }
    }
}
#[derive(Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct GhostTag {
    pub name: String,
    pub is_core: bool,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct VcpOptions {
    pub pipeline: nous_vcp::pipeline::PipelineConfig,
    pub readout: nous_vcp::river::NativeConfig,
    pub candidate_limit: usize,
    pub core_keys: Vec<String>,
    pub ghost_tags: Vec<GhostTag>,
    pub include_trace: bool,
    pub include_fields: bool,
    pub thinking: bool,
    pub thinking_min_closure: f64,
}
impl Default for VcpOptions {
    fn default() -> Self {
        Self {
            pipeline: Default::default(),
            readout: Default::default(),
            candidate_limit: 100,
            core_keys: vec![],
            ghost_tags: vec![],
            include_trace: false,
            include_fields: false,
            thinking: false,
            thinking_min_closure: nous_vcp::thinking::default_closure(),
        }
    }
}
impl VcpOptions {
    pub(super) fn into_config(self, ghost_vectors: Option<Vec<Vec<f32>>>) -> Result<VcpConfig> {
        let vectors = ghost_vectors.unwrap_or_default();
        ensure!(
            vectors.len() == self.ghost_tags.len(),
            "ghost_vectors rows must match vcp.ghost_tags metadata"
        );
        Ok(VcpConfig {
            pipeline: self.pipeline,
            readout: self.readout,
            candidate_limit: self.candidate_limit,
            core_keys: self.core_keys,
            ghost_tags: self
                .ghost_tags
                .into_iter()
                .zip(vectors)
                .map(|(tag, vector)| nous_vcp::pipeline::GhostTagInput {
                    name: tag.name,
                    is_core: tag.is_core,
                    vector,
                })
                .collect(),
            include_trace: self.include_trace,
            include_fields: self.include_fields,
            thinking: self.thinking,
            thinking_min_closure: self.thinking_min_closure,
        })
    }
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueryOptions {
    pub limit: usize,
    pub lexical_weight: f64,
    pub dense_weight: f64,
    pub rrf_k: f64,
    pub vcp: VcpOptions,
}
impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            limit: 20,
            lexical_weight: 1.0,
            dense_weight: 1.0,
            rrf_k: 60.0,
            vcp: VcpOptions::default(),
        }
    }
}
pub fn schemas() -> Value {
    json!({"store_options":schemars::schema_for!(StoreOptions),"assets":schemars::schema_for!(Assets),"query_options":schemars::schema_for!(QueryOptions),"vcp_pipeline":schemars::schema_for!(nous_vcp::pipeline::PipelineConfig),"vcp_readout":schemars::schema_for!(nous_vcp::river::NativeConfig),"nousql":schemars::schema_for!(nousql_syntax::QuerySyntax)})
}
