// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;
use nous_core::{MaintenanceClaim, OperationId, WorkflowSnapshot};
use nous_subject::{CognitiveSeedInput, CreateSubject};
use test_support::{workflow_payload as payload, workflow_snapshot as snapshot};

#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one workflow scenario covers reservation concurrency, proposal recovery and terminal replay"
)]
async fn workflow_reservation_conflict_proposal_resume_and_outcome_replay() {
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
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"original"})),
            360,
        )
        .await
        .unwrap();
    let token = first.lease.as_ref().unwrap().token;
    let concurrent = runtime
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"changed"})),
            360,
        )
        .await
        .unwrap();
    assert!(concurrent.busy);
    assert!(concurrent.lease.is_none());
    assert!(
        runtime
            .store
            .reserve_model_workflow(
                subject,
                "memory",
                &key,
                "different input",
                &snapshot(serde_json::json!({})),
                360
            )
            .await
            .is_err()
    );
    runtime
        .store
        .save_model_workflow(
            &test_support::workflow_lease(subject, "memory", &key, token),
            Some(&payload(serde_json::json!({"text":"validated proposal"}))),
            None,
            None,
            &[],
        )
        .await
        .unwrap();
    runtime
        .store
        .release_model_workflow(&test_support::workflow_lease(
            subject, "memory", &key, token,
        ))
        .await
        .unwrap();
    let resumed = runtime
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({"model":"changed"})),
            360,
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
        .save_model_workflow(
            &test_support::workflow_lease(
                subject,
                "memory",
                &key,
                resumed.lease.as_ref().unwrap().token,
            ),
            None,
            Some(&outcome),
            None,
            &[],
        )
        .await
        .unwrap();
    let completed = runtime
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "same input",
            &snapshot(serde_json::json!({})),
            360,
        )
        .await
        .unwrap();
    assert_eq!(completed.outcome, Some(outcome));
    assert!(completed.proposal.is_none());
    assert!(completed.lease.is_none());
    assert_eq!(completed.snapshot, WorkflowSnapshot::default());
    assert_owner_validation(&runtime.store, subject, &key).await;
    assert_maintenance_workflow_keeps_parent_opportunity(&runtime, subject).await;
    assert_child_result_purge_before_host_ack(&runtime, subject).await;
}

async fn assert_child_result_purge_before_host_ack(
    runtime: &nous_kernel::NousRuntime,
    subject: nous_core::SubjectId,
) {
    let parent = OperationId::new();
    let child = OperationId::new();
    let key = parent.0.to_string();
    let reserved = runtime
        .store
        .reserve_model_workflow(
            subject,
            "memory",
            &key,
            "child commit",
            &snapshot(serde_json::json!({"model":"frozen"})),
            360,
        )
        .await
        .unwrap();
    let lease = reserved.lease.unwrap();
    runtime
        .store
        .save_model_workflow(
            &lease,
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
        .find_model_workflow(subject, "memory", &key, "child commit")
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
            .save_model_workflow(
                &lease,
                Some(&payload(serde_json::json!({"text":"resurrect"}))),
                None,
                None,
                &[]
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn legacy_envelope_migration_preserves_payload_time_unknown_usage_and_declared_dependencies()
{
    let (root, url, _postgres) = test_support::database().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let source = test_support::observation(&runtime, subject, "legacy source").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(test_support::form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "opaque context control",
        ))
        .await
        .unwrap();
    let opaque_id = memory.object.memory_id.0.to_string();
    let key = OperationId::new().0.to_string();
    let need_id = runtime
        .cognition
        .enqueue_maintenance(nous_runtime::MaintenanceRequest {
            subject,
            kind: "memory_consolidate".into(),
            scope_kind: "subject".into(),
            scope_ref: subject.0.to_string(),
            trigger_authority_seq: runtime.store.authority_seq(subject).await.unwrap(),
            due_at: chrono::Utc::now(),
            priority: 1,
        })
        .await
        .unwrap();
    let claim = runtime
        .cognition
        .lease_maintenance(subject, &["memory_consolidate".into()], 1, 60, None)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(claim.need_id, need_id);
    let formed = chrono::Utc::now();
    let saved = serde_json::json!({
        "cognitive_formed_at": formed,
        "maintenance_claim": {"need_id":need_id,"lease_token":claim.lease_token,"trigger":claim.trigger_authority_seq,"trigger_revision":claim.trigger_revision},
        "model": {"context":{"kind":"memory","value":opaque_id}},
        "plan": {"reference":{"kind":"occurrence","value":source.occurrence.occurrence_id}},
    });
    let proposed = serde_json::json!({"text":opaque_id,"context":{"memoryId":opaque_id}});
    // Recreate exactly the pre-transition workflow table shape in this isolated cluster.
    sqlx::raw_sql("ALTER TABLE model_workflow_operations DROP COLUMN dependencies; ALTER TABLE model_workflow_operations DROP COLUMN mutation_operations;").execute(runtime.store.pool()).await.unwrap();
    sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot,proposal,execution_telemetry,maintenance_need_id,maintenance_trigger_revision) VALUES($1,'memory',$2,'legacy',$3,$4,$5,$6,$7)")
        .bind(subject.0).bind(&key).bind(&saved).bind(&proposed)
        .bind(serde_json::json!({"attempts":[{"executionProfile":"route","modelProfile":"model","status":"unknown","latencyMs":12,"failureClass":"caller_cancelled"}]}))
        .bind(need_id).bind(claim.trigger_revision as i64).execute(runtime.store.pool()).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../crates/persistence/migrations/0005_workflow_envelope.sql"
    ))
    .execute(runtime.store.pool())
    .await
    .unwrap();
    let found = runtime
        .store
        .find_model_workflow(subject, "memory", &key, "legacy")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.snapshot.cognitive_formed_at, Some(formed));
    let migrated_claim = found.snapshot.maintenance_claim.as_ref().unwrap();
    assert_eq!(migrated_claim.need_id, need_id);
    assert_eq!(migrated_claim.lease_token, claim.lease_token.unwrap());
    assert_eq!(migrated_claim.trigger_revision, claim.trigger_revision);
    let mut domain = saved;
    domain
        .as_object_mut()
        .unwrap()
        .remove("cognitive_formed_at");
    domain.as_object_mut().unwrap().remove("maintenance_claim");
    assert_eq!(found.snapshot.content.payload, domain);
    assert_eq!(found.proposal.as_ref().unwrap().payload, proposed);
    let execution = found.execution_telemetry.unwrap();
    assert_eq!(
        execution.attempts[0].status,
        nous_core::AttemptStatus::Unknown
    );
    assert!(execution.attempts[0].usage.is_none());
    let mut tx = runtime.store.begin().await.unwrap();
    nous_persistence::AuthorityStore::purge_workflows_in(&mut tx, subject, &[opaque_id])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        runtime
            .store
            .find_model_workflow(subject, "memory", &key, "legacy")
            .await
            .unwrap()
            .unwrap()
            .proposal
            .is_some(),
        "opaque UUID content is not a dependency"
    );
    let mut tx = runtime.store.begin().await.unwrap();
    nous_persistence::AuthorityStore::purge_workflows_in(
        &mut tx,
        subject,
        &[source.occurrence.occurrence_id.0.to_string()],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(
        runtime
            .store
            .find_model_workflow(subject, "memory", &key, "legacy")
            .await
            .unwrap()
            .unwrap()
            .outcome
            .unwrap()
            .purged
    );
}

async fn assert_maintenance_workflow_keeps_parent_opportunity(
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
        .lease_maintenance(subject, &["memory_consolidate".into()], 1, 60, None)
        .await
        .unwrap();
    let need = needs.iter().find(|need| need.need_id == need_id).unwrap();
    let key = OperationId::new().0.to_string();
    let snapshot = WorkflowSnapshot {
        maintenance_claim: Some(MaintenanceClaim {
            need_id,
            lease_token: need.lease_token.unwrap(),
            trigger_authority_seq: need.trigger_authority_seq,
            trigger_revision: need.trigger_revision,
        }),
        ..Default::default()
    };
    let reservation = runtime
        .store
        .reserve_model_workflow(subject, "memory", &key, "bounded maintenance", &snapshot, 1)
        .await
        .unwrap();
    let workflow_until: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT lease_until FROM model_workflow_operations WHERE subject_id=$1 AND owner='memory' AND operation_key=$2").bind(subject.0).bind(&key).fetch_one(runtime.store.pool()).await.unwrap();
    assert!(
        workflow_until >= need.lease_until.unwrap(),
        "a shorter workflow default must not cancel the already authorized maintenance opportunity"
    );
    runtime
        .store
        .save_model_workflow(
            &test_support::workflow_lease(
                subject,
                "memory",
                &key,
                reservation.lease.as_ref().unwrap().token,
            ),
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
    let generalized = store
        .reserve_model_workflow(
            subject,
            domain.as_str(),
            key,
            "domain input",
            &snapshot(serde_json::json!({})),
            360,
        )
        .await
        .unwrap();
    assert!(generalized.lease.is_some());
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
                .reserve_model_workflow(
                    subject,
                    owner,
                    key,
                    "input",
                    &snapshot(serde_json::json!({})),
                    360
                )
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot) VALUES($1,$2,'invalid','input','{}')").bind(subject.0).bind(owner).execute(store.pool()).await.is_err());
    }
}
