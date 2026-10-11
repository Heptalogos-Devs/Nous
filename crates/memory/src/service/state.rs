//! Shared state checks; domain owners retain their transitions, fences and writes.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, ErrorCode, ErrorOwner, Result};

pub(super) fn fence_epoch(owner: ErrorOwner, actual: i64, expected: i64) -> Result<()> {
    if actual != expected {
        return Err(Error::new(
            owner,
            ErrorCode::StaleRevision,
            "Expected object epoch is stale",
        )
        .with_context("expected_epoch", expected)
        .with_context("actual_epoch", actual));
    }
    Ok(())
}

pub(super) fn require_transition(owner: ErrorOwner, current: &str, from: &str) -> Result<()> {
    if current != from {
        return Err(Error::new(
            owner,
            ErrorCode::FailedPrecondition,
            "Object is not in the required state",
        )
        .with_context("required_state", from));
    }
    Ok(())
}
