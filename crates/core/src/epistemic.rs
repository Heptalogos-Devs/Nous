//! Subject knowledge is a scoped self understanding, never a model capability grant.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainKnowledge {
    pub domain_ref: String,
    pub familiarity: String,
    pub can_do: Vec<String>,
    pub limitations: Vec<String>,
    pub experience_kinds: Vec<String>,
    pub interest_refs: Vec<CognitiveRef>,
}
impl DomainKnowledge {
    pub fn validate(&self) -> Result<()> {
        if self.domain_ref.trim().is_empty()
            || self.domain_ref.len() > 1024
            || ![
                "unfamiliar",
                "aware",
                "conversational",
                "practiced",
                "specialized",
            ]
            .contains(&self.familiarity.as_str())
            || self.can_do.len() > 32
            || self.limitations.len() > 32
            || self.interest_refs.len() > 32
            || self
                .can_do
                .iter()
                .chain(&self.limitations)
                .any(|v| v.trim().is_empty() || v.len() > 8192)
            || self.experience_kinds.len() > 8
            || self.experience_kinds.iter().any(|v| {
                !["lived", "heard", "read", "learned", "practiced", "reported"]
                    .contains(&v.as_str())
            })
            || self.interest_refs.iter().any(|r| {
                !matches!(
                    r,
                    CognitiveRef::DesireRevision(_)
                        | CognitiveRef::ConcernRevision(_)
                        | CognitiveRef::DesiredConditionRevision(_)
                )
            })
        {
            return Err(Error::Invalid("invalid Subject domain knowledge".into()));
        }
        Ok(())
    }
}
