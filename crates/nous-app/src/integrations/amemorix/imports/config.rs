use nous_domain::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportFacadePolicy {
    pub enabled: bool,
    pub poll_interval_ms: u64,
    pub llm_enabled: bool,
    pub vectors: ImportVectorPolicy,
}
impl Default for ImportFacadePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_ms: 1000,
            llm_enabled: true,
            vectors: ImportVectorPolicy::MetadataAllowed,
        }
    }
}
impl ImportFacadePolicy {
    pub(crate) fn validate(&self) -> Result<()> {
        if !(200..=86_400_000).contains(&self.poll_interval_ms) {
            return Err(Error::invalid(
                "import poll_interval_ms must be 200..86400000",
            ));
        }
        Ok(())
    }
}
