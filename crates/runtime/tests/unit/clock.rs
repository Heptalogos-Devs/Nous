// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

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
