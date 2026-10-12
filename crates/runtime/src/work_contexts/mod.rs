// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use chrono::{DateTime, Utc};
use nous_core::{CognitiveRef, OperationId, SubjectId};
use nous_persistence::{
    MutationReceipt, check_receipt, commit_receipt, database_error as db, lock_operation,
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

impl WorkContextView {
    pub(crate) fn task_text(&self) -> String {
        let mut text = format!(
            "{}\n{}\n{}",
            self.purpose,
            self.unresolved_questions.join("\n"),
            self.context_text
        );
        for (label, value) in [
            ("Constraints", &self.constraints),
            ("Task budget summary", &self.budget_summary),
        ] {
            if !value.is_null() && value.as_object().is_none_or(|object| !object.is_empty()) {
                text.push_str(&format!("\n{label}: {value}"));
            }
        }
        if !self.resume_conditions.is_empty() {
            text.push_str("\nResume conditions:\n");
            text.push_str(&self.resume_conditions.join("\n"));
        }
        text
    }
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
    pub intent: WorkContextIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkContextIntent {
    Fields(WorkContextFields),
    Pin(WorkContextAnchors),
    Unpin(WorkContextAnchors),
    Clear(WorkContextClearScope),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkContextFields {
    pub purpose: Option<String>,
    pub unresolved_questions: Option<Vec<String>>,
    pub constraints: Option<serde_json::Value>,
    pub resume_conditions: Option<Vec<String>>,
    pub budget_summary: Option<serde_json::Value>,
    pub context_text: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkContextAnchors {
    pub cognition_anchors: Vec<CognitiveRef>,
    pub entity_anchors: Vec<EntityRef>,
    pub tag_anchors: Vec<TagId>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WorkContextClearScope {
    Text,
    Anchors,
    All,
}

fn parse_state(value: String) -> Result<WorkContextState> {
    match value.as_str() {
        "open" => Ok(WorkContextState::Open),
        "paused" => Ok(WorkContextState::Paused),
        "ended" => Ok(WorkContextState::Ended),
        _ => Err(Error::Infrastructure("invalid WorkContext state".into())),
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
        return Err(Error::Invalid(
            "WorkContext purpose is out of bounds".into(),
        ));
    }
    validate_text_array("unresolved_questions", questions)?;
    validate_text_array("resume_conditions", resume_conditions)?;
    if serde_json::to_vec(constraints)
        .map_err(|error| Error::Invalid(error.to_string()))?
        .len()
        > 65_536
    {
        return Err(Error::Invalid(
            "WorkContext constraints are too large".into(),
        ));
    }
    if serde_json::to_vec(budget)
        .map_err(|error| Error::Invalid(error.to_string()))?
        .len()
        > 16_384
    {
        return Err(Error::Invalid(
            "WorkContext budget summary is too large".into(),
        ));
    }
    if cognition_anchors.len() > 256 {
        return Err(Error::Invalid(
            "WorkContext reference bound exceeded".into(),
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
            return Err(Error::Invalid(
                "WorkContext cognition_anchors must be exact continuation refs".into(),
            ));
        }
        if !keys.insert(reference.to_string()) {
            return Err(Error::Invalid("duplicate WorkContext reference".into()));
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
        return Err(Error::Invalid(format!("{name} is out of bounds")));
    }
    Ok(())
}
pub(super) fn replay_context(receipt: MutationReceipt) -> Result<Uuid> {
    receipt
        .result_ref
        .ok_or_else(|| Error::Infrastructure("WorkContext receipt has no result".into()))?
        .parse()
        .map_err(|_| Error::Infrastructure("invalid WorkContext receipt".into()))
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
        return Err(Error::Invalid(
            "WorkContext typed context bounds exceeded".into(),
        ));
    }
    let anchors = typed_anchors(entities, tags);
    let unique = anchors
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    if unique.len() != anchors.len() {
        return Err(Error::Invalid("duplicate WorkContext anchor".into()));
    }
    Ok(())
}
