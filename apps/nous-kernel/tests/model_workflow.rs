// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;
use nous_core::{MaintenanceTrigger, OperationId, WorkflowSnapshot};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use test_support::{workflow_payload as payload, workflow_snapshot as snapshot};

#[tokio::test]
async fn workflow_identity_conflict_proposal_resume_and_outcome_replay() {
    let (root, url, _postgres) = test_support::database().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: None,
            operation_id: OperationId::new(),
            cognitive_seed: CognitiveSeedInput {
                text: "schema_version = 1".into(),
                format: nous_subject::COGNITIVE_SEED_FORMAT.into(),
                provenance: serde_json::json!({}),
            },
            metadata: serde_json::json!({}),
            capabilities: None,
        })
        .await
        .unwrap()
        .subject_id;
    let key = OperationId::new().0.to_string();
    let first = runtime
        .store
        .start_model_operation(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"original"})),
        )
        .await
        .unwrap();
    assert_eq!(
        first.snapshot.content.payload,
        serde_json::json!({"model":"original"})
    );
    let concurrent = runtime
        .store
        .start_model_operation(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"changed"})),
        )
        .await
        .unwrap();
    assert_eq!(concurrent.snapshot, first.snapshot);
    assert!(
        runtime
            .store
            .start_model_operation(
                subject,
                "memory",
                &key,
                "different input",
                &snapshot(serde_json::json!({}))
            )
            .await
            .is_err()
    );
    runtime
        .store
        .save_model_operation(
            &test_support::workflow_identity(subject, "memory", &key, "same input"),
            Some(&payload(serde_json::json!({"text":"validated proposal"}))),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    let resumed = runtime
        .store
        .start_model_operation(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"changed"})),
        )
        .await
        .unwrap();
    assert_eq!(
        resumed.snapshot.content.payload,
        serde_json::json!({"model":"original"})
    );
    assert!(resumed.proposal.is_some());
    let outcome = payload(serde_json::json!({"revision":"receipt reference"}));
    runtime
        .store
        .save_model_operation(
            &test_support::workflow_identity(subject, "memory", &key, "same input"),
            None,
            Some(&outcome),
            None,
            &[],
        )
        .await
        .unwrap();
    let completed = runtime
        .store
        .start_model_operation(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({})),
        )
        .await
        .unwrap();
    assert_eq!(completed.outcome, Some(outcome));
    assert!(completed.proposal.is_none());
    assert_eq!(completed.snapshot, WorkflowSnapshot::default());
    assert_owner_validation(&runtime.store, subject, &key).await;
    assert_maintenance_operation_validates_parent_trigger(&runtime, subject).await;
    assert_child_result_purge_before_host_ack(&runtime, subject).await;
}

async fn assert_child_result_purge_before_host_ack(
    runtime: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
) {
    let parent = OperationId::new();
    let child = OperationId::new();
    let key = parent.0.to_string();
    runtime
        .store
        .start_model_operation(
            subject,
            "memory",
            &key,
            "child commit",
            &snapshot(serde_json::json!({"model":"frozen"})),
        )
        .await
        .unwrap();
    let identity = test_support::workflow_identity(subject, "memory", &key, "child commit");
    runtime
        .store
        .save_model_operation(
            &identity,
            Some(&payload(
                serde_json::json!({"text":"private pending proposal"}),
            )),
            None,
            None,
            &[child],
        )
        .await
        .unwrap();
    let source =
        test_support::observation(runtime, subject, "grounding for a child mutation").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(test_support::form_input(
            subject,
            source.occurrence.occurrence_id,
            child,
            "private cognition",
        ))
        .await
        .unwrap();
    runtime
        .require_memory()
        .unwrap()
        .purge_memory(
            subject,
            memory.object.memory_id,
            OperationId::new(),
            memory.object.object_epoch,
        )
        .await
        .unwrap();
    let found = runtime
        .store
        .read_model_operation(subject, "memory", &key, "child commit")
        .await
        .unwrap()
        .unwrap();
    assert!(
        found.outcome.unwrap().purged,
        "the owner commit links its result before the host acknowledges it"
    );
    assert!(found.proposal.is_none());
    assert_eq!(found.snapshot, WorkflowSnapshot::default());
    assert!(
        runtime
            .store
            .save_model_operation(
                &identity,
                Some(&payload(serde_json::json!({"text":"resurrect"}))),
                None,
                None,
                &[]
            )
            .await
            .is_err()
    );
}

async fn assert_maintenance_operation_validates_parent_trigger(
    runtime: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
) {
    let need_id = runtime
        .cognition
        .enqueue_maintenance(nous_runtime::MaintenanceRequest {
            subject,
            kind: "memory_consolidate".into(),
            scope_kind: "subject".into(),
            scope_ref: subject.0.to_string(),
            trigger_authority_seq: 0,
            due_at: chrono::Utc::now(),
            priority: 1,
        })
        .await
        .unwrap();
    let needs = runtime
        .cognition
        .select_maintenance(subject, &["memory_consolidate".into()], 1, None)
        .await
        .unwrap();
    let need = needs.iter().find(|need| need.need_id == need_id).unwrap();
    let key = OperationId::new().0.to_string();
    let snapshot = WorkflowSnapshot {
        maintenance_trigger: Some(MaintenanceTrigger {
            need_id,
            trigger_authority_seq: need.trigger_authority_seq,
            trigger_revision: need.trigger_revision,
        }),
        ..Default::default()
    };
    let mut invalid = snapshot.clone();
    invalid
        .maintenance_trigger
        .as_mut()
        .unwrap()
        .trigger_revision += 1;
    assert!(
        runtime
            .store
            .start_model_operation(subject, "memory", &key, "bounded maintenance", &invalid,)
            .await
            .is_err()
    );
    runtime
        .store
        .start_model_operation(subject, "memory", &key, "bounded maintenance", &snapshot)
        .await
        .unwrap();
    runtime
        .store
        .save_model_operation(
            &test_support::workflow_identity(subject, "memory", &key, "bounded maintenance"),
            Some(&payload(serde_json::json!({"action":"no_change"}))),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
}

async fn assert_owner_validation(
    store: &nous_persistence::AuthorityStore,
    subject: nous_core::SubjectId,
    key: &str,
) {
    use nous_persistence::WorkflowOwner;
    let domain = WorkflowOwner::new("test_domain_42").unwrap();
    assert_eq!(domain.as_str(), "test_domain_42");
    store
        .start_model_operation(
            subject,
            domain.as_str(),
            key,
            "domain input",
            &snapshot(serde_json::json!({})),
        )
        .await
        .unwrap();
    for owner in [
        "",
        "Memory",
        "1domain",
        "domain-name",
        "领域",
        &"a".repeat(65),
    ] {
        assert!(WorkflowOwner::new(owner).is_err());
        assert!(
            store
                .start_model_operation(
                    subject,
                    owner,
                    key,
                    "input",
                    &snapshot(serde_json::json!({}))
                )
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot) VALUES($1,$2,'invalid','input','{}')").bind(subject.0).bind(owner).execute(store.pool()).await.is_err());
    }
}
