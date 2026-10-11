//! Subject cognition time, independent of execution deadlines and worker leases.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Duration, Utc};
use nous_core::{Error, Result, SubjectId};
use std::{collections::HashMap, sync::Mutex};

pub trait CognitiveClock: Send + Sync {
    fn now(&self, subject: SubjectId) -> DateTime<Utc>;
}

#[derive(Default)]
pub struct SystemCognitiveClock;

impl CognitiveClock for SystemCognitiveClock {
    fn now(&self, _subject: SubjectId) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A retained handle supplies the same semantic time after service reopen.
pub struct ManualCognitiveClock {
    initial: DateTime<Utc>,
    subjects: Mutex<HashMap<SubjectId, DateTime<Utc>>>,
}

impl ManualCognitiveClock {
    pub fn new(initial: DateTime<Utc>) -> Self {
        Self {
            initial,
            subjects: Mutex::new(HashMap::new()),
        }
    }

    pub fn advance_to(&self, subject: SubjectId, instant: DateTime<Utc>) -> Result<()> {
        let mut subjects = self
            .subjects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = subjects.entry(subject).or_insert(self.initial);
        if instant < *current {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "CognitiveClock cannot move backwards",
            ));
        }
        *current = instant;
        Ok(())
    }

    pub fn advance_by(&self, subject: SubjectId, duration: Duration) -> Result<()> {
        let mut subjects = self
            .subjects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = subjects.entry(subject).or_insert(self.initial);
        if duration < Duration::zero() {
            return Err(Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "CognitiveClock cannot move backwards",
            ));
        }
        *current = current.checked_add_signed(duration).ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Runtime,
                nous_core::ErrorCode::InvalidRequest,
                "CognitiveClock advance exceeds timestamp range",
            )
        })?;
        Ok(())
    }
}

impl CognitiveClock for ManualCognitiveClock {
    fn now(&self, subject: SubjectId) -> DateTime<Utc> {
        self.subjects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&subject)
            .copied()
            .unwrap_or(self.initial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_time_is_subject_local_monotonic_and_checked() -> Result<()> {
        let start = DateTime::<Utc>::UNIX_EPOCH;
        let clock = ManualCognitiveClock::new(start);
        let first = SubjectId::new();
        let second = SubjectId::new();
        clock.advance_by(first, Duration::hours(24))?;
        clock.advance_to(first, start + Duration::hours(48))?;
        assert_eq!(clock.now(first), start + Duration::hours(48));
        assert_eq!(clock.now(second), start);
        assert!(clock.advance_to(first, start).is_err());
        assert!(clock.advance_by(first, Duration::seconds(-1)).is_err());
        clock.advance_to(first, DateTime::<Utc>::MAX_UTC)?;
        assert!(clock.advance_by(first, Duration::seconds(1)).is_err());
        assert_eq!(clock.now(first), DateTime::<Utc>::MAX_UTC);
        Ok(())
    }
}
