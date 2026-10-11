//! Shared durable-operation envelope; domain payloads remain owner-defined.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{CognitiveRef, Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkflowPayload {
    pub payload: Value,
    #[serde(default)]
    pub dependencies: Vec<CognitiveRef>,
    #[serde(default)]
    pub purged: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkflowSnapshot {
    pub content: WorkflowPayload,
    pub cognitive_formed_at: Option<DateTime<Utc>>,
    pub maintenance_claim: Option<MaintenanceClaim>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaintenanceClaim {
    pub need_id: Uuid,
    pub lease_token: Uuid,
    pub trigger_authority_seq: i64,
    pub trigger_revision: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Succeeded,
    Failed,
    Skipped,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionAttempt {
    pub execution_profile: String,
    pub model_profile: String,
    pub status: AttemptStatus,
    pub failure_class: Option<String>,
    pub latency_ms: u32,
    pub usage: Option<ExecutionUsage>,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionTelemetry {
    pub attempts: Vec<ExecutionAttempt>,
    pub successful_execution_profile: Option<String>,
    #[serde(default)]
    pub omitted_attempts: u32,
}
impl ExecutionTelemetry {
    pub const ATTEMPT_LIMIT: usize = 64;

    pub fn validate(&self) -> Result<()> {
        const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
        if self.attempts.len() > Self::ATTEMPT_LIMIT
            || self.attempts.iter().any(|attempt| {
                attempt.execution_profile.is_empty()
                    || attempt.execution_profile.len() > 128
                    || attempt.model_profile.len() > 128
                    || attempt
                        .failure_class
                        .as_ref()
                        .is_some_and(|value| value.len() > 256)
                    || attempt.usage.as_ref().is_some_and(|usage| {
                        [usage.input_tokens, usage.output_tokens, usage.total_tokens]
                            .into_iter()
                            .flatten()
                            .any(|value| value > MAX_SAFE_INTEGER)
                    })
            })
        {
            return Err(Error::Invalid("invalid execution telemetry".into()));
        }
        Ok(())
    }

    /// Append each submitted attempt once; retain the earliest 64 and count all omissions.
    pub fn append(&mut self, mut next: Self) {
        let available = Self::ATTEMPT_LIMIT.saturating_sub(self.attempts.len());
        self.omitted_attempts = self
            .omitted_attempts
            .saturating_add(next.omitted_attempts)
            .saturating_add(next.attempts.len().saturating_sub(available) as u32);
        next.attempts.truncate(available);
        self.attempts.extend(next.attempts);
        if next.successful_execution_profile.is_some() {
            self.successful_execution_profile = next.successful_execution_profile;
        }
    }
}
