// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;
use nous_core::{MaintenanceTrigger, ModelResultContext, OperationId};
use test_support::result_payload as payload;

#[tokio::test]
async fn model_operation_identity_conflict_proposal_resume_and_outcome_replay() {
    let (root, url, _postgres) = test_support::database().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let key = OperationId::new().0.to_string();
    let identity = test_support::model_operation_identity(subject, "memory", &key, "same input");
    assert!(
        runtime
            .store
            .read_model_operation(subject, "memory", &key, "same input")
            .await
            .unwrap()
            .is_none()
    );
    runtime
        .store
        .save_model_operation(
            &identity,
            Some(&ModelResultContext::default()),
            Some(&payload(serde_json::json!({"text":"paid proposal"}))),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    let saved = runtime
        .store
        .read_model_operation(subject, "memory", &key, "same input")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.proposal.unwrap().payload,
        serde_json::json!({"text":"paid proposal"})
    );
    assert!(saved.context.is_some());
    assert!(
        runtime
            .store
            .read_model_operation(subject, "memory", &key, "different input")
            .await
            .is_err()
    );
    let outcome = payload(serde_json::json!({"revision":"receipt reference"}));
    runtime
        .store
        .save_model_operation(&identity, None, None, Some(&outcome), None, &[])
        .await
        .unwrap();
    let completed = runtime
        .store
        .read_model_operation(subject, "memory", &key, "same input")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.outcome, Some(outcome));
    assert!(completed.proposal.is_none());
    assert!(completed.context.is_none());
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
        .save_model_operation(
            &test_support::model_operation_identity(subject, "memory", &key, "child commit"),
            Some(&ModelResultContext::default()),
            Some(&test_support::result_payload(
                serde_json::json!({"action":"no_change"}),
            )),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    let identity = test_support::model_operation_identity(subject, "memory", &key, "child commit");
    runtime
        .store
        .save_model_operation(
            &identity,
            None,
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
    assert_eq!(found.context, None);
    assert!(
        runtime
            .store
            .save_model_operation(
                &identity,
                None,
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
    let snapshot = ModelResultContext {
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
            .save_model_operation(
                &test_support::model_operation_identity(
                    subject,
                    "memory",
                    &key,
                    "bounded maintenance"
                ),
                Some(&invalid),
                Some(&test_support::result_payload(
                    serde_json::json!({"action":"no_change"})
                )),
                None,
                None,
                &[]
            )
            .await
            .is_err()
    );
    runtime
        .store
        .save_model_operation(
            &test_support::model_operation_identity(subject, "memory", &key, "bounded maintenance"),
            Some(&snapshot),
            Some(&test_support::result_payload(
                serde_json::json!({"action":"no_change"}),
            )),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    runtime
        .store
        .save_model_operation(
            &test_support::model_operation_identity(subject, "memory", &key, "bounded maintenance"),
            None,
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
    use nous_persistence::ModelResultOwner;
    let domain = ModelResultOwner::new("test_domain_42").unwrap();
    assert_eq!(domain.as_str(), "test_domain_42");
    store
        .save_model_operation(
            &test_support::model_operation_identity(subject, domain.as_str(), key, "domain input"),
            Some(&ModelResultContext::default()),
            Some(&test_support::result_payload(
                serde_json::json!({"action":"no_change"}),
            )),
            None,
            None,
            &[],
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
        assert!(ModelResultOwner::new(owner).is_err());
        assert!(
            store
                .save_model_operation(
                    &test_support::model_operation_identity(subject, owner, key, "input"),
                    Some(&ModelResultContext::default()),
                    Some(&test_support::result_payload(
                        serde_json::json!({"action":"no_change"})
                    )),
                    None,
                    None,
                    &[]
                )
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO model_results(subject_id,owner,operation_key,semantic_digest,context) VALUES($1,$2,'invalid','input','{}')").bind(subject.0).bind(owner).execute(store.pool()).await.is_err());
    }
}
