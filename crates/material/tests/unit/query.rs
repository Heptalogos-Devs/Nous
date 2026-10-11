// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn correlated_axes_do_not_combine_different_origins_or_invent_unknown_times() {
    let early = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let late = early + chrono::Duration::days(10);
    let times = EvidenceTimes {
        sources: vec![
            (TemporalExtent::Instant { at: early }, late, late),
            (TemporalExtent::Instant { at: late }, early, early),
        ],
        formed_at: None,
        ..Default::default()
    };
    let constraints = QueryConstraints {
        occurred: Some(TimePredicate::Range {
            start: None,
            end: Some(late),
        }),
        observed: Some(TimePredicate::Range {
            start: None,
            end: Some(late),
        }),
        ..Default::default()
    };
    assert!(!times.matches(&constraints));
    assert!(!EvidenceTimes::default().matches(&constraints));
    assert!(times.matches(&QueryConstraints::default()));
    assert!(!times.matches(&QueryConstraints {
        valid: Some(TimePredicate::Range {
            start: None,
            end: Some(late)
        }),
        ..Default::default()
    }));
}
