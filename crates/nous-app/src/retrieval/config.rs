use crate::App;
use nous_domain::*;
use nous_foundation::configuration::{
    ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy,
};
use serde::{Deserialize, Serialize};

pub const RETRIEVAL: ConfigKey<RetrievalPolicy> = ConfigKey::new("retrieval");

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RetrievalPolicy {
    pub max_results: usize,
    pub max_query_text_bytes: usize,
    pub max_query_media: usize,
    pub max_snapshot_entries: usize,
    pub hybrid_candidate_multiplier: usize,
    pub max_candidates: usize,
    pub semantic_cache_generations: usize,
    pub embedding_wait_ms: u64,
    pub rrf_constant: f32,
}
impl Default for RetrievalPolicy {
    fn default() -> Self {
        Self {
            max_results: 100,
            max_query_text_bytes: 4096,
            max_query_media: 16,
            max_snapshot_entries: 10000,
            hybrid_candidate_multiplier: 8,
            max_candidates: 800,
            semantic_cache_generations: 8,
            embedding_wait_ms: 45000,
            rrf_constant: 60.0,
        }
    }
}
impl RetrievalPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=10000).contains(&self.max_results)
            || !(1..=1024 * 1024).contains(&self.max_query_text_bytes)
            || !(1..=64).contains(&self.max_query_media)
            || !(1..=100000).contains(&self.max_snapshot_entries)
            || !(1..=64).contains(&self.hybrid_candidate_multiplier)
            || !(self.max_results..=10000).contains(&self.max_candidates)
            || !(1..=64).contains(&self.semantic_cache_generations)
            || !(1..=3600000).contains(&self.embedding_wait_ms)
            || !self.rrf_constant.is_finite()
            || self.rrf_constant < 0.0
        {
            return Err(Error::invalid(
                "retrieval resource limits or fusion constant are out of range",
            ));
        }
        Ok(())
    }
    pub fn admit(&self, request: &QueryRequest) -> Result<()> {
        if request.limit > self.max_results
            || request.input.text.len() > self.max_query_text_bytes
            || request.input.media.len() > self.max_query_media
        {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "query exceeds the configured retrieval policy",
            ));
        }
        if request.mode == QueryMode::Lexical && !request.input.media.is_empty() {
            return Err(Error::invalid(
                "media query input requires dense, vcp or hybrid mode",
            ));
        }
        Ok(())
    }
    pub fn candidates(&self, limit: usize) -> usize {
        limit
            .saturating_mul(self.hybrid_candidate_multiplier)
            .min(self.max_candidates)
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        RETRIEVAL,
        ConfigDefinition {
            owner: "retrieval".into(),
            description: "Query input, snapshot, candidate, cache and rank fusion policy.".into(),
            default: RetrievalPolicy::default(),
            scopes: vec![
                ConfigLevel::Instance,
                ConfigLevel::Namespace,
                ConfigLevel::Subject,
            ],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::Scoped,
        },
        RetrievalPolicy::validate,
    )
}
impl App {
    pub(crate) async fn retrieval_policy(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<RetrievalPolicy> {
        self.configuration
            .snapshot(
                &ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
                false,
            )
            .await?
            .get(RETRIEVAL)
    }
}
