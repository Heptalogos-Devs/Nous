use crate::CognitiveRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolvedQueryIdentity {
    pub reference: CognitiveRef,
    pub label: String,
    pub aliases: Vec<String>,
    pub description: Option<String>,
}
