// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use nous_core::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UseKind {
    Presented,
    Referenced,
    ActedOn,
    ResultSupported,
    ResultRefuted,
    Corrected,
    Pinned,
}

impl UseKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Presented => "presented",
            Self::Referenced => "referenced",
            Self::ActedOn => "acted_on",
            Self::ResultSupported => "result_supported",
            Self::ResultRefuted => "result_refuted",
            Self::Corrected => "corrected",
            Self::Pinned => "pinned",
        }
    }

    pub fn meaningful(self) -> bool {
        !matches!(self, Self::Presented)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveUseEvent {
    pub query_id: Option<uuid::Uuid>,
    pub event_id: UseEventId,
    pub subject_id: SubjectId,
    pub session_id: Option<SessionId>,
    pub reference: CognitiveRef,
    pub use_kind: UseKind,
    pub consumer_ref: String,
    pub occurred_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub context: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveSession {
    pub session_id: SessionId,
    pub subject_id: SubjectId,
    pub opened_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub last_meaningful_use_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub runtime_revision: i64,
    pub active_work_context_id: Option<uuid::Uuid>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentRef {
    pub session_id: SessionId,
    pub reference: CognitiveRef,
    pub entered_at: DateTime<Utc>,
    pub entry_reason: String,
    pub last_meaningful_use_at: Option<DateTime<Utc>>,
    pub hold_until: Option<DateTime<Utc>>,
    pub state: ResidentState,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentState {
    Resident,
    Provisional,
    Evicted,
}

impl ResidentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resident => "resident",
            Self::Provisional => "provisional",
            Self::Evicted => "evicted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResidentAdmission {
    pub reference: CognitiveRef,
    pub reason: String,
    pub hold_until: Option<DateTime<Utc>>,
    pub state: ResidentState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentMutationOutcome {
    pub changed: bool,
    pub runtime_revision: i64,
    pub admitted_count: u32,
    pub evicted_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub session_id: SessionId,
    pub subject_id: SubjectId,
    pub opened_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub last_meaningful_use_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub runtime_revision: i64,
    pub active_work_context_id: Option<uuid::Uuid>,
    pub resident: Vec<ResidentView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentView {
    pub reference: CognitiveRef,
    pub entry_reason: String,
    pub state: String,
    pub entered_at: DateTime<Utc>,
    pub last_meaningful_use_at: Option<DateTime<Utc>>,
    pub hold_until: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseFeedback {
    #[serde(default)]
    pub subject: SubjectId,
    pub session_id: Option<SessionId>,
    pub consumer_ref: String,
    pub events: Vec<UseFeedbackEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseFeedbackEvent {
    #[serde(default)]
    pub query_id: Option<uuid::Uuid>,
    pub event_id: UseEventId,
    pub reference: CognitiveRef,
    pub use_kind: UseKind,
    pub occurred_at: DateTime<Utc>,
    #[serde(default)]
    pub context: serde_json::Value,
}
