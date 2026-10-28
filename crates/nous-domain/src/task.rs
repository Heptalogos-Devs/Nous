//! Persistent task lifecycle shared by model and non-model handlers.
use crate::{Error, NamespaceId, Result, SubjectId, TraceId, validate_name};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    pub requires_attention: bool,
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
    pub source_read: Vec<crate::SourceReadScope>,
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
    pub fn read_scope(&self) -> JobReadScope {
        JobReadScope {
            trace_id: self.trace_id.clone(),
            namespace_id: self.namespace_id.clone(),
            subject_id: self.subject_id.clone(),
            source_read: self.source_read.clone(),
        }
    }
    pub fn require_read(&self, ctx: &crate::CallContext) -> Result<()> {
        self.read_scope().require_read(ctx)
    }
    pub fn call_context(&self, caller_id: impl Into<String>) -> Result<crate::CallContext> {
        self.read_scope().call_context(caller_id)
    }

    pub fn subject_scope(&self) -> Result<(&NamespaceId, &SubjectId)> {
        self.namespace_id
            .as_ref()
            .zip(self.subject_id.as_ref())
            .ok_or_else(|| Error::invalid("task requires a Namespace and Subject scope"))
    }
    pub fn validate(&self) -> Result<()> {
        for scope in &self.source_read {
            scope.validate()?;
            if self.subject_id.as_ref() != Some(&scope.subject_id) {
                return Err(Error::invalid(
                    "task source-read scope differs from its subject",
                ));
            }
        }
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

/// Authorization projection shared by task summaries and full task reads.
#[derive(Debug, Clone)]
pub struct JobReadScope {
    pub trace_id: TraceId,
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub source_read: Vec<crate::SourceReadScope>,
}
impl JobReadScope {
    pub fn require_read(&self, ctx: &crate::CallContext) -> Result<()> {
        ctx.require_accepted_source_read(&self.call_context("task-read")?)?;
        match (&self.namespace_id, &self.subject_id) {
            (Some(namespace), Some(subject)) => ctx.require_subject(namespace, subject),
            (Some(namespace), None) => ctx.require_namespace(namespace),
            _ => ctx.require_admin(),
        }
    }
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
        let mut context = crate::CallerIdentity {
            caller_id: caller_id.into(),
            scope,
            authorization: None,
        }
        .begin_call(self.trace_id.clone());
        context.source_read = self.source_read.clone();
        Ok(context)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct JobListRequest {
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub kind: Option<String>,
    pub state: Option<JobState>,
    pub limit: usize,
    pub cursor: Option<JobCursor>,
}
impl Default for JobListRequest {
    fn default() -> Self {
        Self {
            namespace_id: None,
            subject_id: None,
            kind: None,
            state: None,
            limit: 50,
            cursor: None,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobCursor {
    pub snapshot_seq: i64,
    pub before_commit_seq: i64,
    pub before_job_id: String,
    pub filter_hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: String,
    pub trace_id: TraceId,
    pub namespace_id: Option<NamespaceId>,
    pub subject_id: Option<SubjectId>,
    pub kind: String,
    pub state: JobState,
    pub accepted_commit_seq: i64,
    pub waiting_for: Option<String>,
    pub error: Option<Error>,
    pub created_at: String,
    pub updated_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobPage {
    pub snapshot_seq: i64,
    pub read_at_commit_seq: i64,
    pub items: Vec<JobSummary>,
    pub next_cursor: Option<JobCursor>,
}
