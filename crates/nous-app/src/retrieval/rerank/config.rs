use nous_domain::{Error, QueryRerankMode, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RerankPolicy {
    pub mode: QueryRerankMode,
    pub max_candidates: usize,
    pub max_document_bytes: usize,
    pub max_input_bytes: usize,
    pub wait_ms: u64,
}
impl Default for RerankPolicy {
    fn default() -> Self {
        Self {
            mode: QueryRerankMode::Off,
            max_candidates: 32,
            max_document_bytes: 2048,
            max_input_bytes: 24 * 1024,
            wait_ms: 45000,
        }
    }
}
impl RerankPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(2..=256).contains(&self.max_candidates)
            || !(1..=1024 * 1024).contains(&self.max_document_bytes)
            || !(1..=1024 * 1024).contains(&self.max_input_bytes)
            || !(1..=3600000).contains(&self.wait_ms)
        {
            return Err(Error::invalid(
                "rerank candidate, byte or waiting limits are out of range",
            ));
        }
        Ok(())
    }
}
