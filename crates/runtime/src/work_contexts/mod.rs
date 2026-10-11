// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use chrono::{DateTime, Utc};
use nous_core::{CognitiveRef, OperationId, SubjectId};
use nous_persistence::{
    MutationEnvelope, MutationReceipt, MutationResult, MutationStart, database_error as db,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::BTreeSet;
use uuid::Uuid;

mod lifecycle;
mod mutation;
mod read;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextState {
    Open,
    Paused,
    Ended,
}

impl WorkContextState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Paused => "paused",
            Self::Ended => "ended",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkContextView {
    pub work_context_id: Uuid,
    pub subject_id: SubjectId,
    pub state: WorkContextState,
    pub purpose: String,
    pub unresolved_questions: Vec<String>,
    pub constraints: serde_json::Value,
    pub resume_conditions: Vec<String>,
    pub budget_summary: serde_json::Value,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub cognition_anchors: Vec<CognitiveRef>,
    pub context_text: String,
    pub entity_anchors: Vec<EntityRef>,
    pub tag_anchors: Vec<TagId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkContextInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub purpose: String,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub constraints: serde_json::Value,
    #[serde(default)]
    pub resume_conditions: Vec<String>,
    #[serde(default)]
    pub budget_summary: serde_json::Value,
    #[serde(default)]
    pub cognition_anchors: Vec<CognitiveRef>,
    pub context_text: String,
    pub entity_anchors: Vec<EntityRef>,
    pub tag_anchors: Vec<TagId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWorkContextInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub work_context_id: Uuid,
    pub expected_revision: i64,
    pub purpose: String,
    pub unresolved_questions: Vec<String>,
    pub constraints: serde_json::Value,
    pub resume_conditions: Vec<String>,
    pub budget_summary: serde_json::Value,
    pub cognition_anchors: Vec<CognitiveRef>,
    pub context_text: String,
    pub entity_anchors: Vec<EntityRef>,
    pub tag_anchors: Vec<TagId>,
}

fn parse_state(value: String) -> Result<WorkContextState> {
    match value.as_str() {
        "open" => Ok(WorkContextState::Open),
        "paused" => Ok(WorkContextState::Paused),
        "ended" => Ok(WorkContextState::Ended),
        _ => Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InternalFailure,
            "invalid WorkContext state",
        )),
    }
}

fn validate_payload(
    purpose: &str,
    questions: &[String],
    constraints: &serde_json::Value,
    resume_conditions: &[String],
    budget: &serde_json::Value,
    cognition_anchors: &[CognitiveRef],
) -> Result<()> {
    if purpose.is_empty() || purpose.len() > 8192 {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "WorkContext purpose is out of bounds",
        ));
    }
    validate_text_array("unresolved_questions", questions)?;
    validate_text_array("resume_conditions", resume_conditions)?;
    if serde_json::to_vec(constraints)
        .map_err(|error| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "WorkContext constraints could not be encoded",
            )
            .with_source(error)
        })?
        .len()
        > 65_536
    {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "WorkContext constraints are too large",
        ));
    }
    if serde_json::to_vec(budget)
        .map_err(|error| {
            Error::new(
                ErrorOwner::Runtime,
                ErrorCode::InvalidRequest,
                "WorkContext budget summary could not be encoded",
            )
            .with_source(error)
        })?
        .len()
        > 16_384
    {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "WorkContext budget summary is too large",
        ));
    }
    if cognition_anchors.len() > 256 {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "WorkContext reference bound exceeded",
        ));
    }
    let mut keys = BTreeSet::new();
    for reference in cognition_anchors {
        if !matches!(
            reference,
            CognitiveRef::MemoryRevision(_)
                | CognitiveRef::CognitiveSchemaRevision(_)
                | CognitiveRef::Occurrence(_)
                | CognitiveRef::EpisodeRevision(_)
                | CognitiveRef::JournalRevision(_)
        ) {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "WorkContext cognition_anchors must be exact continuation refs",
            ));
        }
        if !keys.insert(reference.to_string()) {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "duplicate WorkContext reference",
            ));
        }
    }
    Ok(())
}

fn validate_text_array(name: &str, values: &[String]) -> Result<()> {
    if values.len() > 64
        || values
            .iter()
            .any(|value| value.is_empty() || value.len() > 2048)
    {
        return Err(Error::new(
            ErrorOwner::Runtime,
            ErrorCode::InvalidRequest,
            "WorkContext text field is out of bounds",
        )
        .with_context("field", name));
    }
    Ok(())
}
pub(super) fn replay_context(receipt: MutationReceipt) -> Result<Uuid> {
    match receipt.results.first().map(|result| &result.reference) {
        Some(CognitiveRef::WorkContext(id)) => Ok(id.0),
        _ => Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InternalFailure,
            "WorkContext receipt has no typed result",
        )),
    }
}
fn typed_anchors(entities: &[EntityRef], tags: &[TagId]) -> Vec<CognitiveRef> {
    entities
        .iter()
        .cloned()
        .map(CognitiveRef::Entity)
        .chain(tags.iter().copied().map(CognitiveRef::Tag))
        .collect()
}
fn validate_anchors(text: &str, entities: &[EntityRef], tags: &[TagId]) -> Result<()> {
    if text.len() > 65536 || entities.len() > 256 || tags.len() > 256 {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "WorkContext typed context bounds exceeded",
        ));
    }
    let anchors = typed_anchors(entities, tags);
    let unique = anchors
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    if unique.len() != anchors.len() {
        return Err(Error::new(
            nous_core::ErrorOwner::Runtime,
            nous_core::ErrorCode::InvalidRequest,
            "duplicate WorkContext anchor",
        ));
    }
    Ok(())
}
