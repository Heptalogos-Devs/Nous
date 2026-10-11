// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn continuity_and_idle_context_boundaries() {
    let policy = EpisodePolicy {
        soft_idle: Duration::minutes(5),
        hard_idle: Duration::minutes(30),
        context_switch_count: 2,
    };
    let at = DateTime::<Utc>::UNIX_EPOCH;
    let previous = ExperienceContext {
        session: SessionId::new(),
        work_context: None,
        conversation: Some("chat:a".into()),
        actor: None,
        source_class: "message".into(),
    };
    let mut next = previous.clone();
    next.session = SessionId::new();
    assert_eq!(
        policy.boundary(&previous, &next, at, at + Duration::minutes(1), false),
        None
    );
    assert_eq!(
        policy.boundary(&previous, &next, at, at + Duration::minutes(5), false),
        Some("soft_idle_context_switch")
    );
    next = previous.clone();
    assert_eq!(
        policy.boundary(&previous, &next, at, at + Duration::minutes(30), false),
        Some("hard_idle")
    );
    next.actor = Some("person:b".into());
    next.conversation = Some("chat:b".into());
    assert_eq!(
        policy.boundary(&previous, &next, at, at, false),
        Some("context_switch")
    );
    assert_eq!(
        policy.boundary(&previous, &previous, at, at, true),
        Some("work_context_ended")
    );
}
