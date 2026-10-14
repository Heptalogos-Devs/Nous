//! Shared state checks; domain owners retain their transitions, fences and writes.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{DomainError, DomainErrorCode, Error, Result};

pub(super) fn fence_epoch(actual: i64, expected: i64) -> Result<()> {
    if actual != expected {
        return Err(DomainError::new(
            DomainErrorCode::StaleRevision,
            "Expected object epoch is stale",
        )
        .with_context("expected_epoch", expected)
        .with_context("actual_epoch", actual)
        .into());
    }
    Ok(())
}

pub(super) fn require_transition(current: &str, from: &str) -> Result<()> {
    if current != from {
        return Err(Error::FailedPrecondition(format!(
            "object is not in {from} state"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub enum LifecycleAction {
    Suppress,
    Restore,
    Withdraw,
    Reaccept,
}

impl LifecycleAction {
    pub(super) fn transition(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Suppress => ("suppression_state", "normal", "suppressed"),
            Self::Restore => ("suppression_state", "suppressed", "normal"),
            Self::Withdraw => ("acceptance_state", "accepted", "withdrawn"),
            Self::Reaccept => ("acceptance_state", "withdrawn", "accepted"),
        }
    }
}
