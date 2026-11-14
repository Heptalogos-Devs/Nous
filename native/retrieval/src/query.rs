//! One observation feeds candidate recall, ranked readout and grounded Thinking.
mod delivery;
mod localize;
mod observation;
mod program;
mod ranked;
mod recall;
use crate::{
    CancellationToken, failure,
    snapshot::{Snapshot, slice, unit},
};
use anyhow::{Context, Result, ensure};
use nous_vcp::{
    config::QueryControls,
    dtsc::*,
    pipeline::{GhostTagInput, PipelineConfig, PipelineInput, PipelineOutput, run_pipeline},
    river::{CandidateInput, NativeConfig, PreparedPool, ReadoutRequest, prepare_pool, readout},
    thinking::{Stage, ThinkingError, ThinkingInput, plan},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// An ordered, snapshot-bound pool. Readout never runs recall for this value.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSet {
    #[serde(default = "candidate_rank_scope")]
    pub rank_scope: String,
    pub identity: Value,
    pub entries: Vec<PoolEntry>,
    #[serde(default)]
    pub offered_candidates: usize,
    #[serde(default)]
    pub lane_coverage: BTreeMap<String, crate::grouping::LaneCoverage>,
}
fn candidate_rank_scope() -> String {
    "candidate_lane".into()
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoolEntry {
    pub row: u64,
    pub lanes: BTreeMap<String, LaneScore>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LaneScore {
    pub score: f64,
    pub rank: usize,
    #[serde(default = "one_weight")]
    pub weight: f64,
}
fn one_weight() -> f64 {
    1.0
}
#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub grouping: String,
    pub passages_per_group: usize,
    pub candidates_per_group: usize,
    pub ann_search_effort: usize,
    pub group_scan_limit: usize,
    #[serde(skip)]
    pub lane_coverage: std::sync::Mutex<BTreeMap<String, crate::grouping::LaneCoverage>>,
    pub query_space_signature: Option<String>,
    pub top_k: usize,
    pub rrf_k: f64,
    pub preference_weight: f64,
    pub include_trace: bool,
    pub core_rows: Vec<u64>,
    pub ghosts: Vec<Ghost>,
    pub pipeline: PipelineConfig,
    pub readout: NativeConfig,
    pub dtsc: ReferenceDtscConfig,
    pub thinking_stages: Vec<ThinkingStage>,
    pub explore: bool,
    pub thinking_min_closure: f64,
    pub dedup_threshold: Option<f64>,
    pub include_counterevidence: bool,
    pub local_units_per_group: usize,
    pub localize_signals: Vec<String>,
    pub localize_rrf_k: f64,
    pub localize_lexical_weight: f64,
    pub localize_semantic_weight: f64,
    pub context_lexical: Vec<ContextLexical>,
    pub localization_context: Vec<ContextLexical>,
    pub sort_values: BTreeMap<String, BTreeMap<String, Option<SortTime>>>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SortTime {
    pub domain: String,
    pub ticks: i64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextLexical {
    pub key: String,
    pub text: String,
    pub contribution: f64,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ghost {
    pub name: String,
    #[serde(default)]
    pub is_core: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingStage {
    pub name: String,
    pub candidate_rows: Vec<u64>,
    pub top_k: usize,
}
impl Default for Options {
    fn default() -> Self {
        let controls = QueryControls::default();
        Self {
            query_space_signature: None,
            grouping: "candidate".into(),
            passages_per_group: 1,
            candidates_per_group: controls.candidates_per_group,
            ann_search_effort: controls.ann_search_effort,
            group_scan_limit: controls.group_scan_limit,
            lane_coverage: Default::default(),
            top_k: controls.top_k,
            rrf_k: controls.rrf_k,
            preference_weight: controls.preference_weight,
            include_trace: false,
            core_rows: vec![],
            ghosts: vec![],
            pipeline: Default::default(),
            readout: Default::default(),
            dtsc: Default::default(),
            thinking_stages: vec![],
            explore: false,
            thinking_min_closure: controls.thinking_min_closure,
            dedup_threshold: None,
            include_counterevidence: true,
            local_units_per_group: 64,
            localize_signals: vec!["lexical".into(), "semantic".into()],
            localize_rrf_k: 60.0,
            localize_lexical_weight: 1.0,
            localize_semantic_weight: 1.0,
            context_lexical: vec![],
            localization_context: vec![],
            sort_values: BTreeMap::new(),
        }
    }
}
impl Options {
    fn validate_grouping(&self) -> Result<()> {
        ensure!(
            matches!(self.grouping.as_str(), "candidate" | "document")
                && self.passages_per_group > 0
                && (1..=10000).contains(&self.candidates_per_group),
            "Invalid result grouping"
        );
        ensure!(
            (1..=1_000_000).contains(&self.group_scan_limit),
            "group_scan_limit must be in 1..1000000"
        );
        Ok(())
    }
    fn lane_limit(&self, lane: &str) -> usize {
        match lane {
            "query_knn" => self.readout.query_k,
            "denoised_field_knn" => self.readout.denoised_k,
            "local_field_knn" => self.readout.local_field_k,
            "transfer_field_knn" => self.readout.transfer_field_k,
            "bm25" => self.readout.bm25_k,
            "direct_anchor" => self.readout.anchor_k,
            _ => 0,
        }
    }
}
pub struct CalibratedPool {
    pub candidates: CandidateSet,
    pub prepared: PreparedPool,
    pub observation: Arc<PipelineOutput>,
    pub population: usize,
}
pub struct QueryObservation {
    pub output: Arc<PipelineOutput>,
    pub identity: Value,
    pub text: String,
    pub core_rows: Vec<u64>,
    pub records_enabled: bool,
    pub diagnostics_json: String,
}
