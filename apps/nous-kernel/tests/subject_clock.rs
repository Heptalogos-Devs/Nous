// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
mod test_support;
use chrono::{Duration, Utc};
use nous_core::{OperationId, SubjectId};
use nous_runtime::{ClockChange, CognitiveClock, SubjectClockService, SystemCognitiveClock};
use std::sync::Arc;
use test_support::{database, open_runtime, query::subject};

async fn assert_authenticated_sdk(runtime: &nous_kernel::NousRuntime) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(
        nous_kernel::transport::router(runtime.clone(), "clock-fixture".into())
            .await
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    let transport = tokio::process::Command::new("node")
        .args([
            "--import",
            "tsx",
            "packages/client/tests/clock-transport.ts",
            &endpoint,
        ])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .await
        .unwrap();
    assert!(
        transport.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&transport.stdout),
        String::from_utf8_lossy(&transport.stderr)
    );
    server.abort();
}

async fn assert_real_observation(runtime: &nous_kernel::NousRuntime, subject: SubjectId) {
    let accepted =
        test_support::observation(runtime, subject, "real message while world is accelerated")
            .await;
    let occurrence = runtime
        .material
        .occurrence(subject, accepted.occurrence.occurrence_id)
        .await
        .unwrap();
    assert!((occurrence.observed_at - Utc::now()).num_seconds().abs() < 2);
    assert!((occurrence.created_at - Utc::now()).num_seconds().abs() < 2);
}

async fn configure_and_advance(
    runtime: &nous_kernel::NousRuntime,
    subject: SubjectId,
) -> nous_runtime::ClockSnapshot {
    let other = test_support::query::subject(runtime).await;
    let initial = runtime.clocks.read(subject).await.unwrap();
    assert_eq!(initial.time_domain_ref, "time:real");
    let future = Utc::now() + Duration::days(365);
    let configured = runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Configure {
                time_domain_ref: "time:course".into(),
                rate: 3600.0,
                world_time: Some(future),
            },
        )
        .await
        .unwrap();
    assert_real_observation(runtime, subject).await;
    let real = Utc::now();
    assert!(
        (runtime.cognition.now(subject) - real).num_seconds().abs() < 2,
        "legacy owners remain explicitly real-domain"
    );
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let accelerated = runtime.clocks.read(subject).await.unwrap();
    assert!(accelerated.world_now > configured.world_now + Duration::seconds(30));
    assert!((accelerated.recorded_at - Utc::now()).num_seconds().abs() < 2);
    assert_eq!(
        runtime.clocks.read(other).await.unwrap().time_domain_ref,
        "time:real"
    );
    let paused = runtime
        .clocks
        .change(subject, OperationId::new(), ClockChange::Pause)
        .await
        .unwrap();
    let advance_operation = OperationId::new();
    let advanced = runtime
        .clocks
        .change(
            subject,
            advance_operation,
            ClockChange::Advance { seconds: 86400.0 },
        )
        .await
        .unwrap();
    let replay = runtime
        .clocks
        .change(
            subject,
            advance_operation,
            ClockChange::Advance { seconds: 86400.0 },
        )
        .await
        .unwrap();
    assert_eq!(replay.world_now, advanced.world_now);
    assert_eq!(replay.revision, advanced.revision);
    assert_eq!(advanced.world_now, paused.world_now + Duration::days(1));
    assert!(
        runtime
            .clocks
            .change(
                subject,
                advance_operation,
                ClockChange::Advance { seconds: 1.0 }
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .clocks
            .change(
                subject,
                OperationId::new(),
                ClockChange::Advance { seconds: -1.0 }
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .clocks
            .change(
                subject,
                OperationId::new(),
                ClockChange::Configure {
                    time_domain_ref: "time:other".into(),
                    rate: 1.0,
                    world_time: None
                }
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .clocks
            .change(
                subject,
                OperationId::new(),
                ClockChange::Configure {
                    time_domain_ref: "time:course".into(),
                    rate: 1.0,
                    world_time: Some(configured.world_now)
                }
            )
            .await
            .is_err()
    );
    advanced
}

async fn assert_checkpoint_rollback(runtime: &nous_kernel::NousRuntime, subject: SubjectId) {
    let before: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT checkpoint_real FROM subject_clocks WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let mut tx = runtime.store.begin().await.unwrap();
    runtime.clocks.capture_in(&mut tx, subject).await.unwrap();
    tx.rollback().await.unwrap();
    let after: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT checkpoint_real FROM subject_clocks WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "uncommitted owner time does not publish a checkpoint"
    );
}

#[tokio::test]
async fn durable_clock_preserves_world_and_real_time_across_process_restart() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    assert_authenticated_sdk(&runtime).await;
    let subject = subject(&runtime).await;
    let advanced = configure_and_advance(&runtime, subject).await;
    assert_checkpoint_rollback(&runtime, subject).await;
    let mut tx = runtime.store.begin().await.unwrap();
    let stamp = runtime.clocks.capture_in(&mut tx, subject).await.unwrap();
    assert_eq!(stamp.at, advanced.world_now);
    assert_eq!(stamp.time_domain_ref, "time:course");
    assert!((stamp.recorded_at - Utc::now()).num_seconds().abs() < 2);
    // Commit the owner's world stamp and the watermark in the same transaction.
    sqlx::query(
        "CREATE TABLE clock_fixture_owner(subject_id uuid PRIMARY KEY,stamp jsonb NOT NULL)",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO clock_fixture_owner VALUES($1,$2)")
        .bind(subject.0)
        .bind(serde_json::to_value(&stamp).unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut foreign = stamp.clone();
    foreign.time_domain_ref = "time:unknown".into();
    assert_eq!(stamp.compare(&foreign), None);
    drop(runtime);
    // A distinct OS process opens the same actual PostgreSQL state. No retained handle.
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "clock_process_resume_worker", "--nocapture"])
        .env("NOUS_CLOCK_TEST_URL", &url)
        .env("NOUS_CLOCK_TEST_SUBJECT", subject.0.to_string())
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let runtime = open_runtime(&url, &root).await;
    let resumed = runtime
        .clocks
        .change(
            subject,
            OperationId::new(),
            ClockChange::Resume { rate: 60.0 },
        )
        .await
        .unwrap();
    assert_eq!(resumed.world_now, advanced.world_now);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        runtime.clocks.read(subject).await.unwrap().world_now
            > resumed.world_now + Duration::milliseconds(500)
    );
    let checkpoint: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT checkpoint_world FROM subject_clocks WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(runtime.store.pool())
            .await
            .unwrap();
    // Move only the durable real checkpoint backwards to model an hour offline.
    sqlx::query("UPDATE subject_clocks SET checkpoint_real=checkpoint_real-interval '1 hour' WHERE subject_id=$1").bind(subject.0).execute(runtime.store.pool()).await.unwrap();
    let reopened = SubjectClockService::open(runtime.store.clone(), Arc::new(SystemCognitiveClock))
        .await
        .unwrap();
    let offline = reopened.read(subject).await.unwrap();
    let elapsed = offline.world_now - checkpoint;
    assert!(
        (3599..3605).contains(&elapsed.num_seconds()),
        "offline runs at 1x, not active 60x: {elapsed}"
    );
}

#[tokio::test]
async fn clock_process_resume_worker() {
    let Ok(url) = std::env::var("NOUS_CLOCK_TEST_URL") else {
        return;
    };
    let subject = SubjectId(
        std::env::var("NOUS_CLOCK_TEST_SUBJECT")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let store =
        nous_persistence::AuthorityStore::connect(&url, 2, std::time::Duration::from_secs(10))
            .await
            .unwrap();
    let saved: serde_json::Value =
        sqlx::query_scalar("SELECT stamp FROM clock_fixture_owner WHERE subject_id=$1")
            .bind(subject.0)
            .fetch_one(store.pool())
            .await
            .unwrap();
    let stamp: nous_core::DomainStamp = serde_json::from_value(saved).unwrap();
    let clock = SubjectClockService::open(store.clone(), Arc::new(SystemCognitiveClock))
        .await
        .unwrap();
    let now = clock.read(subject).await.unwrap();
    assert_eq!(
        now.world_now, stamp.at,
        "explicit pause survives real process restart"
    );
    // A future real checkpoint simulates a wall clock step backwards at restart.
    sqlx::query("UPDATE subject_clocks SET checkpoint_real=checkpoint_real+interval '1 day',rate=60 WHERE subject_id=$1").bind(subject.0).execute(store.pool()).await.unwrap();
    let reopened = SubjectClockService::open(store.clone(), Arc::new(SystemCognitiveClock))
        .await
        .unwrap();
    assert!(
        reopened.now(subject) >= stamp.at,
        "committed world time never goes backwards"
    );
    // Restore the real checkpoint without changing semantic time.
    sqlx::query(
        "UPDATE subject_clocks SET checkpoint_real=clock_timestamp(),rate=0 WHERE subject_id=$1",
    )
    .bind(subject.0)
    .execute(store.pool())
    .await
    .unwrap();
}
