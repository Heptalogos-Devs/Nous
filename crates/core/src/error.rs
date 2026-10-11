//! Owner-assigned business failures; messages are for display only.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainErrorCode {
    UnknownReference,
    AmbiguousReference,
    ReferenceTypeMismatch,
    ReferenceTombstoned,
    UnresolvedMachinePlaceholder,
    StaleContext,
    StaleRevision,
    OperationIdConflict,
    OperationInProgress,
    LeaseLost,
    CapabilityUnavailable,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct DomainError {
    pub code: DomainErrorCode,
    pub message: String,
    pub context: BTreeMap<String, String>,
}

impl DomainError {
    pub fn new(code: DomainErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            context: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_context(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.context.insert(key.into(), value.to_string());
        self
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    FailedPrecondition(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Infrastructure(String),
    #[error("{0}")]
    Internal(String),
}

pub type Result<T> = std::result::Result<T, Error>;
