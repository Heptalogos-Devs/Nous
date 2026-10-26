//! Persistent task lifecycle shared by model and non-model handlers.
use crate::{Error, NamespaceId, Result, SubjectId, TraceId, validate_name};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

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
    pub resume: ResumeCondition,
    pub details: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResumeCondition {
    Ready,
    NotBefore { unix_ms: i64 },
    ReadyOrNotBefore { unix_ms: i64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: String,
    pub trace_id: TraceId,
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub kind: String,
    pub dependencies: Vec<String>,
    pub state: JobState,
    pub wait: Option<WaitCondition>,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}

impl JobRecord {
    pub fn call_context(&self, caller_id: impl Into<String>) -> Result<crate::CallContext> {
        let scope = match (&self.namespace_id, &self.subject_id) {
            (None, None) => crate::AccessScope::Admin,
            (Some(namespace_id), None) => crate::AccessScope::Namespace {
                namespace_id: namespace_id.clone(),
            },
            (Some(namespace_id), Some(subject_id)) => crate::AccessScope::Subject {
                namespace_id: namespace_id.clone(),
                subject_id: subject_id.clone(),
            },
            (None, Some(_)) => return Err(Error::invalid("Subject task requires a Namespace")),
        };
        Ok(crate::CallerIdentity {
            caller_id: caller_id.into(),
            scope,
            authorization: None,
        }
        .begin_call(self.trace_id.clone()))
    }

    pub fn subject_scope(&self) -> Result<(&NamespaceId, &SubjectId)> {
        self.namespace_id
            .as_ref()
            .zip(self.subject_id.as_ref())
            .ok_or_else(|| Error::invalid("task requires a Namespace and Subject scope"))
    }
    pub fn validate(&self) -> Result<()> {
        self.trace_id.validate()?;
        validate_name(&self.id, "job ID")?;
        validate_name(&self.kind, "task kind")?;
        let mut dependencies = BTreeSet::new();
        for dependency in &self.dependencies {
            validate_name(dependency, "task dependency")?;
            if dependency == &self.id || !dependencies.insert(dependency) {
                return Err(Error::invalid(
                    "task dependencies must be distinct other tasks",
                ));
            }
        }
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
            if matches!(wait.resume,ResumeCondition::NotBefore{unix_ms}|ResumeCondition::ReadyOrNotBefore{unix_ms} if unix_ms<0)
            {
                return Err(Error::invalid("task resume deadline must be nonnegative"));
            }
            if !wait.details.is_object() {
                return Err(Error::invalid("wait condition details must be an object"));
            }
            if wait.kind == "dependencies"
                && (self.dependencies.is_empty() || wait.details != serde_json::json!({}))
            {
                return Err(Error::invalid(
                    "dependency waiting requires task dependencies and empty details",
                ));
            }
        }
        validate_name(&self.created_at, "job created_at")?;
        validate_name(&self.updated_at, "job updated_at")?;
        Ok(())
    }
}
