//! Owner-assigned business failures; messages are safe for display only.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
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
    InvalidRequest,
    NotFound,
    Conflict,
    FailedPrecondition,
    Unavailable,
    InternalFailure,
    CapacityExhausted,
    InstanceAlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorOwner {
    Identity,
    Subject,
    Memory,
    Material,
    Runtime,
    Configuration,
    Concept,
    Observation,
    Resource,
    ModelWorkflow,
    Kernel,
    Core,
    Persistence,
    Instance,
    Cli,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub owner: ErrorOwner,
    pub code: ErrorCode,
    pub message: String,
    pub context: BTreeMap<String, String>,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(owner: ErrorOwner, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            owner,
            code,
            message: message.into(),
            context: BTreeMap::new(),
            source: None,
        }
    }

    #[must_use]
    pub fn with_context(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        self.context.insert(key.into(), value.to_string());
        self
    }

    #[must_use]
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }
}

pub type Result<T> = std::result::Result<T, Error>;
