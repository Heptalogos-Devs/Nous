// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

#[path = "test_support/mod.rs"]
mod test_support;

use chrono::{Duration, Utc};
use nous_core::OperationId;
use nous_material::{
    ObservationInput, ObservationMaterial, OccurrenceDescriptor, RuntimeDirective,
};
use nous_protocol::public as p;
use nous_subject::{CognitiveSeedInput, CreateSubject};
use serde_json::json;
use uuid::Uuid;

fn request(identity: p::get_operation_status_request::Identity) -> p::GetOperationStatusRequest {
    p::GetOperationStatusRequest {
        identity: Some(identity),
    }
}

async fn lookup(
    service: &nous_kernel::transport::KernelService,
    request: p::GetOperationStatusRequest,
) -> p::GetOperationStatusResponse {
    use p::system_service_server::SystemService;

    SystemService::get_operation_status(service, tonic::Request::new(request))
        .await
        .expect("operation status lookup")
        .into_inner()
}

#[tokio::test]
async fn operation_status_reads_owner_rows_and_keeps_workflow_outcome_opaque() {
    let (root, url, _postgres) = test_support::database().await;
    let runtime = test_support::open_runtime(&url, &root).await;
    let subject_operation = OperationId::new();
    let subject = runtime
        .subjects
        .create_subject(CreateSubject {
            subject_id: nous_core::SubjectId::new(),
            operation_id: subject_operation,
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
    let service = nous_kernel::transport::KernelService(runtime.clone());

    let created = lookup(
        &service,
        request(p::get_operation_status_request::Identity::SubjectCreate(
            p::SubjectCreateIdentity {
                subject_id: subject.0.to_string(),
                operation_id: subject_operation.0.to_string(),
            },
        )),
    )
    .await;
    assert_eq!(created.state, p::OperationState::Committed as i32);
    assert_eq!(created.owner, p::ErrorOwner::Subject as i32);
    assert_eq!(
        created.result_reference,
        Some(p::CognitiveRef {
            kind: "subject".into(),
            value: subject.0.to_string(),
        })
    );
    assert!(created.recorded_at.is_some());

    let mutation_id = Uuid::new_v4();
    let result_id = Uuid::new_v4();
    let revision_id = Uuid::new_v4();
    let recorded_at = Utc::now() - Duration::seconds(3);
    let committed_at = Utc::now();
    sqlx::query("INSERT INTO mutation_receipts(subject_id,operation_id,operation_kind,request_digest,state,created_at) VALUES($1,$2,'form_memory',$3,'in_progress',$4)")
        .bind(subject.0)
        .bind(mutation_id)
        .bind("a".repeat(64))
        .bind(recorded_at)
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let mutation_in_progress = lookup(
        &service,
        request(p::get_operation_status_request::Identity::MutationReceipt(
            p::MutationReceiptIdentity {
                subject_id: subject.0.to_string(),
                operation_id: mutation_id.to_string(),
                operation_kind: "form_memory".into(),
            },
        )),
    )
    .await;
    assert_eq!(
        mutation_in_progress.state,
        p::OperationState::InProgress as i32
    );
    assert!(mutation_in_progress.committed_at.is_none());
    sqlx::query("UPDATE mutation_receipts SET state='committed',result_ref_kind='memory',result_ref=$3,result_revision=$4,result_epoch=7,committed_at=$5 WHERE subject_id=$1 AND operation_id=$2")
        .bind(subject.0)
        .bind(mutation_id)
        .bind(result_id.to_string())
        .bind(revision_id)
        .bind(committed_at)
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let mutation = lookup(
        &service,
        request(p::get_operation_status_request::Identity::MutationReceipt(
            p::MutationReceiptIdentity {
                subject_id: subject.0.to_string(),
                operation_id: mutation_id.to_string(),
                operation_kind: "form_memory".into(),
            },
        )),
    )
    .await;
    assert_eq!(mutation.state, p::OperationState::Committed as i32);
    assert_eq!(mutation.owner, p::ErrorOwner::Memory as i32);
    assert_eq!(mutation.subject_id, Some(subject.0.to_string()));
    assert_eq!(
        mutation.result_reference,
        Some(p::CognitiveRef {
            kind: "memory".into(),
            value: result_id.to_string(),
        })
    );
    assert_eq!(mutation.result_revision_id, Some(revision_id.to_string()));
    assert_eq!(mutation.result_epoch, Some(7));
    assert_eq!(
        mutation.recorded_at.unwrap().seconds,
        recorded_at.timestamp()
    );
    assert_eq!(
        mutation.committed_at.unwrap().seconds,
        committed_at.timestamp()
    );

    let failed_id = Uuid::new_v4();
    sqlx::query("INSERT INTO mutation_receipts(subject_id,operation_id,operation_kind,request_digest,state,created_at,terminal_problem_code) VALUES($1,$2,'form_memory',$3,'failed_terminal',$4,'STALE_REVISION')")
        .bind(subject.0)
        .bind(failed_id)
        .bind("e".repeat(64))
        .bind(Utc::now())
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let failed = lookup(
        &service,
        request(p::get_operation_status_request::Identity::MutationReceipt(
            p::MutationReceiptIdentity {
                subject_id: subject.0.to_string(),
                operation_id: failed_id.to_string(),
                operation_kind: "form_memory".into(),
            },
        )),
    )
    .await;
    assert_eq!(failed.state, p::OperationState::FailedTerminal as i32);
    assert_eq!(failed.terminal_code.as_deref(), Some("STALE_REVISION"));

    let missing = lookup(
        &service,
        request(p::get_operation_status_request::Identity::MutationReceipt(
            p::MutationReceiptIdentity {
                subject_id: subject.0.to_string(),
                operation_id: Uuid::new_v4().to_string(),
                operation_kind: "form_memory".into(),
            },
        )),
    )
    .await;
    assert_eq!(missing.state, p::OperationState::Unknown as i32);
    assert_eq!(missing.owner, p::ErrorOwner::Memory as i32);
    let invalid_kind = p::system_service_server::SystemService::get_operation_status(
        &service,
        tonic::Request::new(request(
            p::get_operation_status_request::Identity::MutationReceipt(
                p::MutationReceiptIdentity {
                    subject_id: subject.0.to_string(),
                    operation_id: Uuid::new_v4().to_string(),
                    operation_kind: "future_operation".into(),
                },
            ),
        )),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid_kind.code(), tonic::Code::InvalidArgument);

    let runtime_operation_id = Uuid::new_v4();
    sqlx::query("INSERT INTO mutation_receipts(subject_id,operation_id,operation_kind,request_digest,state,created_at) VALUES($1,$2,'create_work_context',$3,'in_progress',$4)")
        .bind(subject.0)
        .bind(runtime_operation_id)
        .bind("f".repeat(64))
        .bind(Utc::now())
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let runtime_receipt = lookup(
        &service,
        request(p::get_operation_status_request::Identity::MutationReceipt(
            p::MutationReceiptIdentity {
                subject_id: subject.0.to_string(),
                operation_id: runtime_operation_id.to_string(),
                operation_kind: "create_work_context".into(),
            },
        )),
    )
    .await;
    assert_eq!(runtime_receipt.state, p::OperationState::InProgress as i32);
    assert_eq!(runtime_receipt.owner, p::ErrorOwner::Runtime as i32);

    let config_id = Uuid::new_v4();
    sqlx::query("INSERT INTO configuration_mutation_receipts(operation_id,request_digest,subject_id,key,action,revision,apply_mode,pending_restart,active_digest,desired_digest,created_at) VALUES($1,$2,NULL,'core_execution.enabled','set',2,'live',false,$3,$4,$5)")
        .bind(config_id)
        .bind("b".repeat(64))
        .bind("c".repeat(64))
        .bind("d".repeat(64))
        .bind(Utc::now())
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let config = lookup(
        &service,
        request(
            p::get_operation_status_request::Identity::ConfigurationMutation(
                p::ConfigurationMutationIdentity {
                    operation_id: config_id.to_string(),
                },
            ),
        ),
    )
    .await;
    assert_eq!(config.state, p::OperationState::Committed as i32);
    assert_eq!(config.owner, p::ErrorOwner::Configuration as i32);
    assert!(config.subject_id.is_none());

    let request_id = Uuid::new_v4();
    let observed = runtime
        .material
        .record_observation_once(
            ObservationInput {
                subject,
                session: None,
                occurrence: OccurrenceDescriptor {
                    source_class: nous_core::SourceClass::Message,
                    external_object_ref: None,
                    occurred_time: nous_core::TemporalExtent::Unknown,
                    observed_at: Some(Utc::now()),
                    conversation_ref: None,
                    actor_entity_ref: None,
                    context: json!({}),
                },
                material: ObservationMaterial::InlineText {
                    text: "status test observation".into(),
                    media_type: "text/plain".into(),
                },
                entities: Vec::new(),
                runtime: RuntimeDirective::default(),
            },
            Some(request_id),
        )
        .await
        .unwrap();
    let observation = lookup(
        &service,
        request(
            p::get_operation_status_request::Identity::ObservationRequest(
                p::ObservationRequestIdentity {
                    subject_id: subject.0.to_string(),
                    request_id: request_id.to_string(),
                },
            ),
        ),
    )
    .await;
    assert_eq!(observation.state, p::OperationState::Committed as i32);
    assert_eq!(observation.owner, p::ErrorOwner::Observation as i32);
    assert_eq!(
        observation.result_reference,
        Some(p::CognitiveRef {
            kind: "occurrence".into(),
            value: observed.occurrence.occurrence_id.0.to_string(),
        })
    );

    let workflow_key = "opaque-terminal-outcome";
    let workflow_lease_until = Utc::now() + Duration::minutes(1);
    sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot,lease_until) VALUES($1,'memory',$2,'semantic-digest',$3,$4)")
        .bind(subject.0)
        .bind(workflow_key)
        .bind(json!({"private_snapshot":"must not escape"}))
        .bind(workflow_lease_until)
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let workflow_in_progress = lookup(
        &service,
        request(p::get_operation_status_request::Identity::ModelWorkflow(
            p::ModelWorkflowIdentity {
                subject_id: subject.0.to_string(),
                owner: "memory".into(),
                operation_key: workflow_key.into(),
            },
        )),
    )
    .await;
    assert_eq!(
        workflow_in_progress.state,
        p::OperationState::InProgress as i32
    );
    assert_eq!(
        workflow_in_progress.lease_until.unwrap().seconds,
        workflow_lease_until.timestamp()
    );
    let opaque_outcome = json!({"status":"committed","result_reference":{"kind":"memory","value":result_id.to_string()}});
    sqlx::query("UPDATE model_workflow_operations SET outcome=$3,updated_at=$4,lease_until=NULL WHERE subject_id=$1 AND owner='memory' AND operation_key=$2")
        .bind(subject.0)
        .bind(workflow_key)
        .bind(opaque_outcome)
        .bind(Utc::now())
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let workflow = lookup(
        &service,
        request(p::get_operation_status_request::Identity::ModelWorkflow(
            p::ModelWorkflowIdentity {
                subject_id: subject.0.to_string(),
                owner: "memory".into(),
                operation_key: workflow_key.into(),
            },
        )),
    )
    .await;
    assert_eq!(workflow.state, p::OperationState::Completed as i32);
    assert_eq!(workflow.owner, p::ErrorOwner::Memory as i32);
    assert!(workflow.result_reference.is_none());
    assert!(workflow.result_revision_id.is_none());
    assert!(workflow.lease_until.is_none());
    assert_eq!(
        workflow.updated_at.unwrap().seconds,
        workflow.recorded_at.unwrap().seconds
    );

    let material_workflow_key = "material-workflow";
    sqlx::query("INSERT INTO model_workflow_operations(subject_id,owner,operation_key,semantic_digest,snapshot) VALUES($1,'material',$2,'semantic-digest',$3)")
        .bind(subject.0)
        .bind(material_workflow_key)
        .bind(json!({}))
        .execute(runtime.store.pool())
        .await
        .unwrap();
    let material_workflow = lookup(
        &service,
        request(p::get_operation_status_request::Identity::ModelWorkflow(
            p::ModelWorkflowIdentity {
                subject_id: subject.0.to_string(),
                owner: "material".into(),
                operation_key: material_workflow_key.into(),
            },
        )),
    )
    .await;
    assert_eq!(material_workflow.owner, p::ErrorOwner::Material as i32);
    assert_eq!(
        material_workflow.state,
        p::OperationState::InProgress as i32
    );

    let invalid = p::system_service_server::SystemService::get_operation_status(
        &service,
        tonic::Request::new(request(
            p::get_operation_status_request::Identity::ModelWorkflow(p::ModelWorkflowIdentity {
                subject_id: subject.0.to_string(),
                owner: "unknown-owner".into(),
                operation_key: workflow_key.into(),
            }),
        )),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
}
