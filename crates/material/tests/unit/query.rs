// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
fn query(constraints: QueryConstraints) -> CognitiveQuery {
    serde_json::from_value(serde_json::json!({"session":null,"expression":CognitiveQueryExpr {constraints,..Default::default()}})).unwrap()
}
fn source(at: DateTime<Utc>, observed: DateTime<Utc>) -> SourceTemporalEvidence {
    SourceTemporalEvidence {
        occurrence_id: OccurrenceId::new(),
        occurred: DomainTemporalExtent::real(TemporalExtent::Instant { at }),
        observed: DomainStamp {
            time_domain_ref: "time:real".into(),
            at: observed,
            recorded_at: observed,
        },
        observed_at: observed,
        recorded_at: observed,
    }
}
#[test]
fn correlated_axes_do_not_combine_different_origins_or_invent_unknown_times() {
    let early = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let late = early + chrono::Duration::days(10);
    let times = EvidenceTimes {
        temporal: TemporalEvidence {
            sources: vec![source(early, late), source(late, early)],
        },
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
    assert!(!times.matches(&query(constraints.clone())));
    assert!(!EvidenceTimes::default().matches(&query(constraints)));
    assert!(times.matches(&query(QueryConstraints::default())));
    assert!(!times.matches(&query(QueryConstraints {
        valid: Some(TimePredicate::Range {
            start: None,
            end: Some(late)
        }),
        ..Default::default()
    })));
}
#[test]
fn source_domains_do_not_compare_by_numeric_equality() {
    let at = DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut fact = source(at, at + chrono::Duration::days(1));
    fact.occurred.time_domain_ref = "time:world".into();
    fact.observed = DomainStamp {
        time_domain_ref: "time:world".into(),
        at,
        recorded_at: fact.recorded_at,
    };
    let times = EvidenceTimes {
        temporal: TemporalEvidence {
            sources: vec![fact],
        },
        ..Default::default()
    };
    let mut input = query(QueryConstraints {
        occurred: Some(TimePredicate::Point { at }),
        observed: Some(TimePredicate::Point { at }),
        ..Default::default()
    });
    assert!(!times.matches(&input));
    input.expression.constraints.time_domains = TimeAxisDomains {
        occurred: Some("time:world".into()),
        observed: Some("time:world".into()),
        ..Default::default()
    };
    assert!(times.matches(&input));
    input.expression.constraints.time_domains.observed = Some("time:real".into());
    assert!(!times.matches(&input));
}
