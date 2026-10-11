// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use nous_configuration::ConfigKey;
use nous_core::OperationId;
use serde_json::json;
use test_support::{database, open_runtime};

const EPSILON: ConfigKey<f64> = ConfigKey::new("memory.accessibility.epsilon");
const TAU_DAYS: ConfigKey<f64> = ConfigKey::new("memory.accessibility.tau_days");

#[tokio::test]
async fn concurrent_configuration_mutations_publish_one_complete_snapshot() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let service = runtime.configuration.clone();
    let first_id = OperationId::new();
    let second_id = OperationId::new();
    let (first, second) = tokio::join!(
        service.set_system_override(first_id, EPSILON.path(), json!(0.11), None),
        service.set_system_override(second_id, TAU_DAYS.path(), json!(42.0), None)
    );
    first.expect("epsilon mutation");
    second.expect("tau mutation");
    let snapshot = service.active_system_snapshot().expect("active snapshot");
    assert_eq!(snapshot.get(EPSILON).expect("epsilon"), 0.11);
    assert_eq!(snapshot.get(TAU_DAYS).expect("tau"), 42.0);
    assert!(
        service
            .set_system_override(first_id, EPSILON.path(), json!(0.12), None)
            .await
            .is_err()
    );

    let revision = service.desired_system_snapshot().unwrap().revision;
    let first_id = OperationId::new();
    let second_id = OperationId::new();
    let (first, second) = tokio::join!(
        service.set_system_override(first_id, EPSILON.path(), json!(0.12), Some(revision)),
        service.set_system_override(second_id, EPSILON.path(), json!(0.13), Some(revision))
    );
    let (winner_id, winner_value, winner) = match (first, second) {
        (Ok(outcome), Err(error)) if error.code == nous_core::ErrorCode::StaleRevision => {
            (first_id, 0.12, outcome)
        }
        (Err(error), Ok(outcome)) if error.code == nous_core::ErrorCode::StaleRevision => {
            (second_id, 0.13, outcome)
        }
        outcomes => panic!("one revision must admit exactly one writer: {outcomes:?}"),
    };
    assert_eq!(winner.revision, revision + 1);
    assert_eq!(
        service
            .active_system_snapshot()
            .unwrap()
            .get(EPSILON)
            .unwrap(),
        winner_value
    );
    assert!(matches!(
        service
            .clear_system_override(OperationId::new(), EPSILON.path(), Some(revision))
            .await,
        Err(nous_core::Error {
            code: nous_core::ErrorCode::StaleRevision,
            ..
        })
    ));
    let later = service
        .set_system_override(
            OperationId::new(),
            TAU_DAYS.path(),
            json!(43.0),
            Some(winner.revision),
        )
        .await
        .unwrap();
    let replay = service
        .set_system_override(
            winner_id,
            EPSILON.path(),
            json!(winner_value),
            Some(revision),
        )
        .await
        .unwrap();
    assert_eq!(replay.revision, winner.revision);
    assert_eq!(replay.active_digest, winner.active_digest);
    assert_eq!(
        service.desired_system_snapshot().unwrap().revision,
        later.revision
    );
    assert_eq!(later.revision, revision + 2);
}
