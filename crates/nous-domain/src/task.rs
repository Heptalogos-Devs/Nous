//! Persistent task lifecycle shared by model and non-model handlers.
use crate::{Error, NamespaceId, Result, SubjectId, validate_name};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Waiting,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitCondition {
    pub kind: String,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub kind: String,
    pub state: JobState,
    pub wait: Option<WaitCondition>,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

impl JobRecord {
    pub fn subject_scope(&self) -> Result<(&NamespaceId, &SubjectId)> {
        self.namespace_id
            .as_ref()
            .zip(self.subject_id.as_ref())
            .ok_or_else(|| Error::invalid("task requires a Namespace and Subject scope"))
    }
    pub fn validate(&self) -> Result<()> {
        validate_name(&self.id, "job ID")?;
        validate_name(&self.kind, "task kind")?;
        if self.subject_id.is_some() && self.namespace_id.is_none() {
            return Err(Error::invalid("Subject task requires a Namespace"));
        }
        if (self.state == JobState::Waiting) != self.wait.is_some() {
            return Err(Error::invalid(
                "waiting task requires exactly one wait condition",
            ));
        }
        if let Some(wait) = &self.wait {
            validate_name(&wait.kind, "wait condition kind")?;
            if !wait.details.is_object() {
                return Err(Error::invalid("wait condition details must be an object"));
            }
        }
        validate_name(&self.created_at, "job created_at")?;
        validate_name(&self.updated_at, "job updated_at")?;
        Ok(())
    }
}
