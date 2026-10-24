//! Frozen geometry inputs and the resulting EPA/AGS assets.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryTag {
    pub id: i64,
    pub name: String,
    pub vector_version: i64,
    pub vector: Vec<f32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryInput {
    pub dimension: usize,
    pub model_sig: String,
    pub config: Value,
    pub tags: Vec<GeometryTag>,
    pub memberships: Vec<crate::artifact::FileTag>,
    pub recorded_at_ms: i64,
}
#[derive(Debug, Serialize)]
pub struct IntrinsicResidual {
    pub id: i64,
    pub status: &'static str,
    pub count: usize,
    pub residual: f64,
}
pub(super) type LocalResult = IntrinsicResidual;
#[derive(Debug, Serialize)]
pub struct GeometryOutput {
    pub algorithm_version: &'static str,
    pub signature: String,
    pub config_hash: String,
    pub tag_count: usize,
    pub computed_count: usize,
    pub intrinsic: Vec<IntrinsicResidual>,
    pub anchor_gain: HashMap<i64, f64>,
    pub mean: Vec<f64>,
    pub basis: Vec<Vec<f64>>,
    pub coverage: Vec<f64>,
    pub epa_cache_json: String,
}
