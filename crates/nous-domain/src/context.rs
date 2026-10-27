//! Persistent caller identity and a single traced invocation have separate lifetimes.
use crate::{Error, ErrorCode, GrantRef, NamespaceId, Result, SourceReadScope, SubjectId, TraceId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    pub authorization: Option<GrantRef>,
}

impl CallerIdentity {
    pub fn begin_call(&self, trace_id: TraceId) -> CallContext {
        CallContext {
            caller_id: self.caller_id.clone(),
            scope: self.scope.clone(),
            trace_id,
            authorization: self.authorization.clone(),
            source_read: Vec::new(),
        }
    }
    pub fn new_call(&self) -> CallContext {
        self.begin_call(TraceId::new())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallContext {
    pub caller_id: String,
    pub scope: AccessScope,
    pub trace_id: TraceId,
    pub authorization: Option<GrantRef>,
    #[serde(default)]
    pub source_read: Vec<SourceReadScope>,
}

impl CallContext {
    pub fn require_accepted_source_read(&self, accepted: &Self) -> Result<()> {
        let accepted = accepted.source_read_identity()?;
        if self
            .source_read_identity()?
            .iter()
            .any(|constraint| !accepted.contains(constraint))
        {
            return Err(self.forbidden("record belongs to a different accepted source-read scope"));
        }
        Ok(())
    }
    pub fn source_read_identity(&self) -> Result<Vec<SourceReadScope>> {
        let mut scopes = self
            .source_read
            .iter()
            .map(|scope| {
                scope.validate()?;
                let scope = scope.canonical();
                Ok((serde_json::to_string(&scope)?, scope))
            })
            .collect::<Result<Vec<_>>>()?;
        scopes.sort_by(|a, b| a.0.cmp(&b.0));
        scopes.dedup_by(|a, b| a.0 == b.0);
        Ok(scopes.into_iter().map(|(_, scope)| scope).collect())
    }
    pub fn with_source_read(&self, scope: SourceReadScope) -> Result<Self> {
        scope.validate()?;
        if let AccessScope::Subject { subject_id, .. } = &self.scope
            && subject_id != &scope.subject_id
        {
            return Err(self.forbidden("source-read scope belongs to another subject"));
        }
        let mut narrowed = self.clone();
        if !narrowed.source_read.contains(&scope) {
            narrowed.source_read.push(scope);
        }
        Ok(narrowed)
    }
    pub fn new(caller_id: impl Into<String>, scope: AccessScope) -> Self {
        CallerIdentity {
            caller_id: caller_id.into(),
            scope,
            authorization: None,
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
            authorization: self.authorization.clone(),
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
