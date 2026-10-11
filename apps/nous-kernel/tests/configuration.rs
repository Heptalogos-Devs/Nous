// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use nous_configuration::{ConfigKey, SUBJECT_DEFAULT_MEMORY};
use nous_core::OperationId;
use nous_subject::{CognitiveSeedInput, CreateSubject};
use serde_json::json;
use test_support::{database, open_runtime};

const EPSILON: ConfigKey<f64> = ConfigKey::new("memory.accessibility.epsilon");
const TAU_DAYS: ConfigKey<f64> = ConfigKey::new("memory.accessibility.tau_days");

#[tokio::test]
async fn missing_serving_cache_rebuilds_without_changing_authority() {
    let (root, url, _postgres) = database().await;
    let runtime = test_support::open_runtime_with_serving(&url, &root, true, false, false).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: json!({}),
            },
            metadata: json!({}),
            capabilities: None,
        })
        .await
        .unwrap()
        .subject_id;
    let observed = test_support::observation(
        &runtime,
        subject,
        "Original research remains readable after Serving cache loss",
    )
    .await;
    let initial = runtime.serving.refresh(subject).await.unwrap();
    assert!(initial.degradation.is_empty(), "{:?}", initial.degradation);
    let before = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.family == "lexical")
        .unwrap();
    drop(runtime);
    std::fs::rename(root.path().join("serving"), root.path().join("old-cache")).unwrap();
    let runtime = test_support::open_runtime_with_serving(&url, &root, true, false, false).await;
    let result = runtime.serving.refresh(subject).await.unwrap();
    assert!(result.degradation.is_empty(), "{:?}", result.degradation);
    let after = runtime
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.family == "lexical")
        .unwrap();
    assert_ne!(before.generation_id, after.generation_id);
    assert_eq!(before.authority_watermark, after.authority_watermark);
    assert!(std::path::Path::new(&after.artifact_location).exists());
    let evidence = runtime
        .materialize(
            subject,
            nous_material::MaterializeRequest {
                reference: nous_core::CognitiveRef::Occurrence(observed.occurrence.occurrence_id),
                byte_range: None,
                resource_handle: None,
                resource: None,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8(evidence.bytes).unwrap(),
        "Original research remains readable after Serving cache loss"
    );
}

#[tokio::test]
async fn configuration_precedence_permissions_and_restart_state_are_explicit() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: json!({}),
            },
            metadata: json!({}),
            capabilities: None,
        })
        .await
        .expect("subject")
        .subject_id;
    let service = &runtime.configuration;
    assert_eq!(
        service
            .snapshot_for_subject(subject)
            .unwrap()
            .get(EPSILON)
            .unwrap(),
        0.02
    );
    service
        .set_system_override(OperationId::new(), EPSILON.path(), json!(0.04), None)
        .await
        .expect("system override");
    assert_eq!(
        service
            .snapshot_for_subject(subject)
            .unwrap()
            .get(EPSILON)
            .unwrap(),
        0.04
    );
    service
        .set_subject_override(
            OperationId::new(),
            subject,
            EPSILON.path(),
            json!(0.08),
            None,
        )
        .await
        .expect("subject override");
    assert_eq!(
        service
            .snapshot_for_subject(subject)
            .unwrap()
            .get(EPSILON)
            .unwrap(),
        0.08
    );
    let active = service.active_system_snapshot().unwrap();
    let outcome = service
        .set_system_override(
            OperationId::new(),
            "runtime.resident_limit",
            json!(512),
            None,
        )
        .await
        .expect("restart override");
    assert!(outcome.pending_restart);
    assert_eq!(
        active.get_path::<usize>("runtime.resident_limit").unwrap(),
        256
    );
    assert_eq!(
        service
            .desired_system_snapshot()
            .unwrap()
            .get_path::<usize>("runtime.resident_limit")
            .unwrap(),
        512
    );
    assert!(
        service
            .snapshot_for_subject(subject)
            .unwrap()
            .get(SUBJECT_DEFAULT_MEMORY)
            .unwrap(),
    );
}

#[tokio::test]
async fn configuration_receipts_freeze_subject_scope_and_replay_outcomes() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: json!({}),
            },
            metadata: json!({}),
            capabilities: None,
        })
        .await
        .expect("subject")
        .subject_id;
    let service = &runtime.configuration;
    let first_id = OperationId::new();
    let first = service
        .set_subject_override(first_id, subject, EPSILON.path(), json!(0.08), None)
        .await
        .expect("first subject override");
    assert_eq!(
        first.active_digest,
        service
            .snapshot_for_subject(subject)
            .unwrap()
            .effective_digest
    );
    assert_eq!(first.active_digest, first.desired_digest);
    assert_ne!(
        first.active_digest,
        service.active_system_snapshot().unwrap().effective_digest
    );

    let later = service
        .set_subject_override(
            OperationId::new(),
            subject,
            EPSILON.path(),
            json!(0.09),
            None,
        )
        .await
        .expect("later subject override");
    let replay = service
        .set_subject_override(first_id, subject, EPSILON.path(), json!(0.08), None)
        .await
        .expect("replay original subject override");
    assert_eq!(replay.revision, first.revision);
    assert_eq!(replay.active_digest, first.active_digest);
    assert_eq!(replay.desired_digest, first.desired_digest);
    assert_ne!(later.active_digest, replay.active_digest);
    assert!(matches!(
        service
            .set_subject_override(first_id, subject, EPSILON.path(), json!(0.1), None)
            .await,
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::OperationIdConflict
    ));
}

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
        (Ok(outcome), Err(nous_core::Error::Domain(error)))
            if error.code == nous_core::DomainErrorCode::StaleRevision =>
        {
            (first_id, 0.12, outcome)
        }
        (Err(nous_core::Error::Domain(error)), Ok(outcome))
            if error.code == nous_core::DomainErrorCode::StaleRevision =>
        {
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
        Err(nous_core::Error::Domain(error)) if error.code == nous_core::DomainErrorCode::StaleRevision
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

#[tokio::test]
async fn rebuild_policy_and_provisioning_defaults_preserve_adopted_subject_state() {
    use nous_configuration::ConfigApplyMode;
    let (root, url, _postgres) = database().await;
    let runtime = test_support::open_runtime_with_serving(&url, &root, false, false, true).await;
    let create = || CreateSubject {
        subject_id: None,
        operation_id: OperationId::new(),
        cognitive_seed: CognitiveSeedInput {
            text: "schema_version = 1".into(),
            format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
            provenance: json!({}),
        },
        metadata: json!({}),
        capabilities: None,
    };
    let old = runtime.subjects.create_subject(create()).await.unwrap();
    runtime.serving.refresh(old.subject_id).await.unwrap();
    let before = runtime
        .store
        .serving_current(old.subject_id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.family == "topology")
        .unwrap();
    let service = nous_kernel::transport::KernelService(runtime.clone());
    let status_request = || {
        tonic::Request::new(nous_protocol::public::SubjectRequest {
            subject_id: old.subject_id.0.to_string(),
        })
    };
    let change = runtime
        .configuration
        .set_system_override(
            OperationId::new(),
            "topology.wave.outbound_budget",
            json!(0.8),
            None,
        )
        .await
        .unwrap();
    assert_eq!(change.apply_mode, ConfigApplyMode::ServingRebuild);
    let stale = nous_protocol::public::system_service_server::SystemService::get_projection_status(
        &service,
        status_request(),
    )
    .await
    .unwrap()
    .into_inner();
    let stale = stale
        .families
        .iter()
        .find(|f| f.name == "topology")
        .unwrap();
    assert_eq!(stale.state, "STALE");
    assert_eq!(
        stale.config_digest.as_ref().unwrap(),
        before.config_digest.as_str()
    );
    assert_ne!(
        stale.config_digest.as_deref(),
        Some(stale.desired_config_digest.as_str())
    );
    let rebuild = runtime.serving.refresh(old.subject_id).await.unwrap();
    assert!(rebuild.rebuilt.contains(&"topology".into()));
    let after = runtime
        .store
        .serving_current(old.subject_id)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.family == "topology")
        .unwrap();
    assert_ne!(before.generation_id, after.generation_id);
    assert_ne!(before.config_digest, after.config_digest);
    let ready = nous_protocol::public::system_service_server::SystemService::get_projection_status(
        &service,
        status_request(),
    )
    .await
    .unwrap()
    .into_inner();
    let ready = ready
        .families
        .iter()
        .find(|f| f.name == "topology")
        .unwrap();
    assert_eq!(ready.state, "READY");
    assert_eq!(
        ready.config_digest.as_deref(),
        Some(ready.desired_config_digest.as_str())
    );
    let provision = runtime
        .configuration
        .set_system_override(
            OperationId::new(),
            SUBJECT_DEFAULT_MEMORY.path(),
            json!(false),
            None,
        )
        .await
        .unwrap();
    assert_eq!(provision.apply_mode, ConfigApplyMode::NewSubjectsOnly);
    let new = runtime.subjects.create_subject(create()).await.unwrap();
    assert!(!new.capabilities.memory);
    assert!(
        runtime
            .subjects
            .subject(old.subject_id)
            .await
            .unwrap()
            .capabilities
            .memory
    );
}
