// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use chrono::{Duration, SubsecRound, Utc};
use nous_runtime::{MaintenanceDisposition, MaintenanceRequest, ManualCognitiveClock};
use std::sync::Arc;
use test_support::database;

use test_support::longitudinal::runtime_with_clock;
use test_support::query::subject as create_subject;

#[tokio::test]
async fn maintenance_selection_preserves_new_triggers_during_work() {
    let (root, url, _postgres) = database().await;
    let start = Utc::now().trunc_subsecs(6);
    let clock = Arc::new(ManualCognitiveClock::new(start));
    let rt = runtime_with_clock(&url, &root, clock.clone()).await;
    let subject = create_subject(&rt).await;
    let request = MaintenanceRequest {
        subject,
        kind: "journal_review".into(),
        scope_kind: "track".into(),
        scope_ref: "interaction".into(),
        trigger_authority_seq: 1,
        due_at: start + Duration::hours(1),
        priority: 30,
    };
    let id = rt
        .cognition
        .enqueue_maintenance(request.clone())
        .await
        .unwrap();
    let kinds = vec!["journal_review".into()];
    assert!(
        rt.cognition
            .select_maintenance(subject, &kinds, 1, None, None)
            .await
            .unwrap()
            .is_empty()
    );
    clock.advance_by(subject, Duration::hours(1)).unwrap();
    assert!(
        rt.cognition
            .select_maintenance(subject, &kinds, 1, None, None)
            .await
            .unwrap()
            .is_empty(),
        "semantic advance does not mature an explicit real scheduler deadline"
    );
    rt.cognition
        .enqueue_maintenance(MaintenanceRequest {
            due_at: rt.cognition.recorded_now(),
            ..request.clone()
        })
        .await
        .unwrap();
    let selected = rt
        .cognition
        .select_maintenance(subject, &kinds, 1, None, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(selected.need_id, id);
    assert_eq!(selected.state, "pending");
    let during_work = rt
        .cognition
        .select_maintenance(subject, &kinds, 1, None, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(during_work.trigger_revision, selected.trigger_revision);
    assert_eq!(during_work.updated_at, selected.updated_at);
    let newer = MaintenanceRequest {
        trigger_authority_seq: 2,
        ..request
    };
    assert_eq!(rt.cognition.enqueue_maintenance(newer).await.unwrap(), id);
    rt.cognition
        .acknowledge_maintenance(&selected, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    let next = rt
        .cognition
        .select_maintenance(subject, &kinds, 1, None, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(next.need_id, id);
    assert_eq!(next.trigger_authority_seq, 2);
    assert!(next.trigger_revision > selected.trigger_revision);
    rt.cognition
        .acknowledge_maintenance(&next, MaintenanceDisposition::Satisfied)
        .await
        .unwrap();
    assert!(
        rt.cognition
            .maintenance_needs(subject)
            .await
            .unwrap()
            .is_empty()
    );
}
