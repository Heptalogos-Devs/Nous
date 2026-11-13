//! Typed build configuration shared by execution and packaged schema/defaults.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
mod artifact;
mod geometry;

pub use artifact::{OrderedCooccurrence, Transport};
pub use geometry::{AnchorGain, GeometryGlobal, GeometryLocal, SemanticGate};

/// Typed parameters for Wave's independently composable numerical operators.
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EmptyOperatorParameters {}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LaneParameters {
    #[schemars(range(min = 0, max = 1000000))]
    pub candidate_limit: Option<usize>,
    #[schemars(range(min = 1, max = 10000))]
    pub candidates_per_group: Option<usize>,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AnnParameters {
    #[schemars(range(min = 0, max = 1000000))]
    pub candidate_limit: Option<usize>,
    #[schemars(range(min = 1, max = 10000))]
    pub candidates_per_group: Option<usize>,
    #[schemars(range(min = 1, max = 1000000))]
    pub search_effort: Option<usize>,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnionParameters {
    #[schemars(range(min = 1, max = 1000000))]
    pub candidate_limit: Option<usize>,
    #[schemars(range(min = 1, max = 1000000))]
    pub group_limit: Option<usize>,
    #[schemars(range(min = 1, max = 10000))]
    pub candidates_per_group: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaneRankParameters {
    pub lane: String,
    #[serde(default)]
    #[schemars(range(min = 1, max = 1000000))]
    pub group_limit: Option<usize>,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ReadoutParameters {
    #[schemars(range(min = 1, max = 1000000))]
    pub group_limit: Option<usize>,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FusionParameters {
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub k: Option<f64>,
    pub weights: Vec<f64>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LocateParameters {
    #[schemars(range(min = 1, max = 1000000))]
    pub local_units_per_group: usize,
    #[schemars(length(min = 1))]
    pub signals: Vec<LocateSignal>,
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub rrf_k: f64,
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub lexical_weight: f64,
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub semantic_weight: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LocateSignal {
    Lexical,
    Semantic,
}
impl LocateSignal {
    pub fn name(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
        }
    }
}

impl Default for LocateParameters {
    fn default() -> Self {
        Self {
            local_units_per_group: 64,
            signals: vec![LocateSignal::Semantic],
            rrf_k: 60.0,
            lexical_weight: 1.0,
            semantic_weight: 1.0,
        }
    }
}
impl LocateParameters {
    pub fn validate(&self) -> Result<(), String> {
        if self.local_units_per_group == 0
            || self.signals.is_empty()
            || self
                .signals
                .iter()
                .enumerate()
                .any(|(index, signal)| self.signals[..index].contains(signal))
            || !self.rrf_k.is_finite()
            || self.rrf_k <= 0.0
            || !self.lexical_weight.is_finite()
            || self.lexical_weight <= 0.0
            || !self.semantic_weight.is_finite()
            || self.semantic_weight <= 0.0
        {
            return Err("Invalid localization parameters".into());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SelectParameters {
    pub budget: Option<DeliveryBudget>,
}

impl Default for SelectParameters {
    fn default() -> Self {
        Self {
            budget: Some(DeliveryBudget {
                amount: 2048,
                unit: DeliveryUnit::Characters,
            }),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeliveryBudget {
    #[schemars(range(min = 1))]
    pub amount: usize,
    pub unit: DeliveryUnit,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryUnit {
    Characters,
    Bytes,
    Tokens,
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct GeometryConfig {
    pub local: GeometryLocal,
    pub global: GeometryGlobal,
}
impl GeometryConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.local.validate()?;
        self.global.validate()?;
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ArtifactConfig {
    pub ordered_cooccurrence: OrderedCooccurrence,
    pub v9: Transport,
}
impl ArtifactConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.ordered_cooccurrence.validate()?;
        self.v9.validate()?;
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct BuildConfig {
    /// Maximum occurrence pairs in each construction pass; null computes all.
    pub pair_work_limit: Option<u64>,
    pub geometry: GeometryConfig,
    pub artifact: ArtifactConfig,
}
impl BuildConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.geometry.validate()?;
        self.artifact.validate()?;
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct QueryControls {
    #[schemars(range(min = 1, max = 1000000))]
    /// Maximum different candidate rows inspected per grouped vector lane.
    pub group_scan_limit: usize,
    #[schemars(range(min = 1, max = 10000))]
    /// Maximum candidates retained per material while recalling and combining lanes.
    pub candidates_per_group: usize,
    #[schemars(range(min = 1, max = 1000000))]
    /// Candidate ANN graph expansion used for each search.
    pub ann_search_effort: usize,
    #[schemars(range(min = 1, max = 10000))]
    pub top_k: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub thinking_min_closure: f64,
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub rrf_k: f64,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub preference_weight: f64,
}
impl Default for QueryControls {
    fn default() -> Self {
        Self {
            top_k: 10,
            group_scan_limit: 4096,
            candidates_per_group: 2,
            ann_search_effort: 32,
            thinking_min_closure: crate::thinking::default_closure(),
            rrf_k: 60.0,
            preference_weight: 0.25,
        }
    }
}
impl QueryControls {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=1000000).contains(&self.group_scan_limit) {
            return Err("group_scan_limit must be in 1..1000000".into());
        }
        if !(1..=10000).contains(&self.candidates_per_group) {
            return Err("candidates_per_group must be in 1..10000".into());
        }
        if !(1..=1000000).contains(&self.ann_search_effort) {
            return Err("ann_search_effort must be in 1..1000000".into());
        }
        if !(1..=10000).contains(&self.top_k) {
            return Err("top_k must be in 1..10000".into());
        }
        if !self.thinking_min_closure.is_finite()
            || !(0.0..=1.0).contains(&self.thinking_min_closure)
        {
            return Err("thinking_min_closure must be finite and in 0..1".into());
        }
        if !self.rrf_k.is_finite() || self.rrf_k <= 0.0 {
            return Err("rrf_k must be finite and positive".into());
        }
        if !self.preference_weight.is_finite() || !(0.0..=1.0).contains(&self.preference_weight) {
            return Err("preference_weight must be finite and in 0..1".into());
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextControls {
    #[schemars(range(min = -1.0, max = 1.0))]
    pub segment_threshold: f64,
    #[schemars(range(min = 1, max = 10000))]
    pub window: usize,
    #[schemars(range(min = 0.0, max = 1.0))]
    pub context_mix: f64,
    /// Unused context mass for the parent-based relevance allocation.
    #[schemars(extend("exclusiveMinimum" = 0.0))]
    pub null_mass: f64,
    #[schemars(extend("additionalProperties" = serde_json::json!({"type":"number","minimum":0.0})))]
    pub role_weights: std::collections::BTreeMap<String, f64>,
    #[schemars(range(min = 0.0))]
    pub default_role_weight: f64,
}
impl Default for ContextControls {
    fn default() -> Self {
        Self {
            window: 10,
            segment_threshold: 0.70,
            context_mix: 0.2,
            null_mass: 1.0,
            role_weights: [
                ("user", 1.0),
                ("assistant", 0.5),
                ("work", 1.0),
                ("ego", 0.25),
                ("source", 1.0),
            ]
            .into_iter()
            .map(|(role, weight)| (role.into(), weight))
            .collect(),
            default_role_weight: 1.0,
        }
    }
}
