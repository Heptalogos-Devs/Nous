//! Immutable access-grant revisions and their controlled delegation rules.
use crate::{AccessScope, CallerIdentity, Error, GrantId, Result, SecretRef, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantRef {
    pub id: GrantId,
    pub revision: i64,
}
impl GrantRef {
    pub fn validate(&self) -> Result<()> {
        self.id.validate()?;
        if self.revision <= 0 {
            return Err(Error::invalid(
                "grant reference requires a positive exact revision",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    Bootstrap,
    Issued,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGrant {
    pub id: GrantId,
    pub kind: GrantKind,
    pub name: String,
    pub caller_id: String,
    pub scope: AccessScope,
    pub parent: Option<GrantRef>,
    pub can_delegate: bool,
    pub expires_at_unix_ms: Option<i64>,
    pub state: GrantState,
    pub credential_hash: String,
    pub secret_ref: SecretRef,
    pub revision: i64,
    pub created_at: String,
    pub recorded_at: String,
    pub actor: String,
    pub reason: String,
    pub commit_seq: i64,
}

impl GrantId {
    pub fn bootstrap() -> Self {
        Self("00000000-0000-4000-8000-000000000001".into())
    }
}

impl AccessGrant {
    pub fn reference(&self) -> GrantRef {
        GrantRef {
            id: self.id.clone(),
            revision: self.revision,
        }
    }
    pub fn identity(&self) -> CallerIdentity {
        CallerIdentity {
            caller_id: self.caller_id.clone(),
            scope: self.scope.clone(),
            authorization: Some(self.reference()),
        }
    }
    pub fn active_at(&self, unix_ms: i64) -> bool {
        self.state == GrantState::Active
            && self
                .expires_at_unix_ms
                .is_none_or(|expires| unix_ms < expires)
    }
    pub fn validate(&self) -> Result<()> {
        self.id.validate()?;
        self.secret_ref.validate()?;
        for (value, field) in [
            (&self.name, "grant name"),
            (&self.caller_id, "grant caller"),
            (&self.actor, "grant actor"),
            (&self.reason, "grant reason"),
            (&self.created_at, "grant created_at"),
            (&self.recorded_at, "grant recorded_at"),
        ] {
            validate_name(value, field)?;
        }
        if self.revision <= 0 || self.revision != self.commit_seq {
            return Err(Error::invalid(
                "grant revision must equal its positive operation sequence",
            ));
        }
        if self.credential_hash.len() != 64
            || !self
                .credential_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::invalid(
                "grant credential hash must be lowercase SHA-256",
            ));
        }
        match &self.scope {
            AccessScope::Admin => {}
            AccessScope::Namespace { namespace_id } => namespace_id.validate()?,
            AccessScope::Subject {
                namespace_id,
                subject_id,
            } => {
                namespace_id.validate()?;
                subject_id.validate()?;
            }
        }
        if let Some(parent) = &self.parent {
            parent.validate()?;
            if parent.id == self.id {
                return Err(Error::invalid("grant cannot delegate to itself"));
            }
        }
        if self.expires_at_unix_ms.is_some_and(|expires| expires < 0) {
            return Err(Error::invalid(
                "grant expiry must be a nonnegative real timestamp",
            ));
        }
        if (self.kind == GrantKind::Bootstrap) != (self.id == GrantId::bootstrap()) {
            return Err(Error::invalid("bootstrap grant has one reserved identity"));
        }
        if self.kind == GrantKind::Bootstrap
            && (!matches!(self.scope, AccessScope::Admin)
                || self.parent.is_some()
                || !self.can_delegate
                || self.expires_at_unix_ms.is_some())
        {
            return Err(Error::invalid(
                "bootstrap grant must be an unbounded delegating administrator",
            ));
        }
        Ok(())
    }
}
