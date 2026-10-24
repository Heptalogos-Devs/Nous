//! Persistent caller identity and a single traced invocation have separate lifetimes.
use crate::{Error, ErrorCode, NamespaceId, Result, SubjectId, TraceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccessScope {
    Admin,
    Namespace {
        namespace_id: NamespaceId,
    },
    Subject {
        namespace_id: NamespaceId,
        subject_id: SubjectId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallerIdentity {
    pub caller_id: String,
    pub scope: AccessScope,
}

impl CallerIdentity {
    pub fn begin_call(&self, trace_id: TraceId) -> CallContext {
        CallContext {
            caller_id: self.caller_id.clone(),
            scope: self.scope.clone(),
            trace_id,
        }
    }
    pub fn new_call(&self) -> CallContext {
        self.begin_call(TraceId::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallContext {
    pub caller_id: String,
    pub scope: AccessScope,
    pub trace_id: TraceId,
}

impl CallContext {
    pub fn new(caller_id: impl Into<String>, scope: AccessScope) -> Self {
        CallerIdentity {
            caller_id: caller_id.into(),
            scope,
        }
        .new_call()
    }
    pub fn admin() -> Self {
        Self::new("admin", AccessScope::Admin)
    }
    pub fn identity(&self) -> CallerIdentity {
        CallerIdentity {
            caller_id: self.caller_id.clone(),
            scope: self.scope.clone(),
        }
    }
    pub fn narrow(&self, scope: AccessScope) -> Result<Self> {
        match &scope {
            AccessScope::Admin => self.require_admin()?,
            AccessScope::Namespace { namespace_id } => self.require_namespace(namespace_id)?,
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } => self.require_subject(namespace_id, subject_id)?,
        }
        Ok(Self {
            scope,
            ..self.clone()
        })
    }
    pub fn require_admin(&self) -> Result<()> {
        match self.scope {
            AccessScope::Admin => Ok(()),
            _ => Err(self.forbidden("administrator scope required")),
        }
    }
    pub fn require_namespace(&self, namespace: &NamespaceId) -> Result<()> {
        match &self.scope {
            AccessScope::Admin => Ok(()),
            AccessScope::Namespace { namespace_id } if namespace_id == namespace => Ok(()),
            _ => Err(self.forbidden("namespace scope required")),
        }
    }
    pub fn require_subject(&self, namespace: &NamespaceId, subject: &SubjectId) -> Result<()> {
        match &self.scope {
            AccessScope::Admin => Ok(()),
            AccessScope::Namespace { namespace_id } if namespace_id == namespace => Ok(()),
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } if namespace_id == namespace && subject_id == subject => Ok(()),
            _ => Err(self.forbidden("subject scope required")),
        }
    }
    fn forbidden(&self, message: &str) -> Error {
        Error::new(ErrorCode::Forbidden, message).with_trace(self.trace_id.clone())
    }
}
