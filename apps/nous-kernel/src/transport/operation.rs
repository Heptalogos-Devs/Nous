// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::CognitiveRef;
use nous_persistence::{MutationResult, database_error as db};
use sqlx::Row;
use sqlx::types::Json;

fn operation_owner(kind: &str) -> Option<p::ErrorOwner> {
    Some(match kind {
        "add_schema_evidence"
        | "create_cognitive_schema"
        | "merge_cognitive_schemas"
        | "revise_cognitive_schema"
        | "split_cognitive_schema" => p::ErrorOwner::Concept,
        "cognition_lifecycle" => p::ErrorOwner::Unspecified,
        "create_episode"
        | "episode_partition"
        | "form_memory"
        | "journal_commit"
        | "link_episode_revisions"
        | "link_memory_revisions"
        | "revise_episode"
        | "revise_memory" => p::ErrorOwner::Memory,
        "create_association" | "revoke_association" | "create_tag" | "revise_tag"
        | "merge_tags" | "split_tag" => p::ErrorOwner::Concept,
        "create_work_context"
        | "set_active_work_context"
        | "update_work_context"
        | "pause_work_context"
        | "resume_work_context"
        | "end_work_context" => p::ErrorOwner::Runtime,
        _ => return None,
    })
}

fn workflow_owner(owner: &str) -> Option<p::ErrorOwner> {
    match owner {
        "memory" => Some(p::ErrorOwner::Memory),
        "material" => Some(p::ErrorOwner::Material),
        _ => None,
    }
}

impl KernelService {
    pub(super) async fn get_operation_status(
        &self,
        input: p::GetOperationStatusRequest,
    ) -> nous_core::Result<p::GetOperationStatusResponse> {
        use p::get_operation_status_request::Identity;

        match input.identity.ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Kernel,
                nous_core::ErrorCode::InvalidRequest,
                "operation identity is required",
            )
        })? {
            Identity::MutationReceipt(identity) => {
                let subject = id(&identity.subject_id)?;
                let operation = id(&identity.operation_id)?;
                let owner = operation_owner(&identity.operation_kind).ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Kernel,
                        nous_core::ErrorCode::InvalidRequest,
                        "operation_kind is not registered",
                    )
                })?;
                let row = sqlx::query("SELECT state,created_at,committed_at,results FROM mutation_receipts WHERE subject_id=$1 AND operation_id=$2 AND operation_kind=$3")
                    .bind(subject)
                    .bind(operation)
                    .bind(&identity.operation_kind)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(db)?;
                let Some(row) = row else {
                    return Ok(p::GetOperationStatusResponse {
                        state: p::OperationState::Unknown as i32,
                        owner: if identity.operation_kind == "cognition_lifecycle" {
                            p::ErrorOwner::Unspecified as i32
                        } else {
                            owner as i32
                        },
                        subject_id: Some(subject.to_string()),
                        ..Default::default()
                    });
                };
                let receipt_state: String = row.try_get("state").map_err(db)?;
                let state = match receipt_state.as_str() {
                    "in_progress" => p::OperationState::InProgress,
                    "committed" => p::OperationState::Committed,
                    _ => {
                        return Err(Error::new(
                            nous_core::ErrorOwner::Kernel,
                            nous_core::ErrorCode::InternalFailure,
                            "invalid operation receipt state",
                        ));
                    }
                };
                let results: Vec<MutationResult> = row
                    .try_get::<Json<Vec<MutationResult>>, _>("results")
                    .map_err(db)?
                    .0;
                let owner = if identity.operation_kind == "cognition_lifecycle" {
                    match results.first().map(|result| &result.reference) {
                        Some(CognitiveRef::CognitiveSchema(_)) => p::ErrorOwner::Concept,
                        Some(_) => p::ErrorOwner::Memory,
                        None => p::ErrorOwner::Unspecified,
                    }
                } else {
                    owner
                };
                let results = results
                    .into_iter()
                    .map(|result| p::OperationResult {
                        reference: Some(to_ref(result.reference)),
                        revision_id: result.revision_id.map(|value| value.to_string()),
                        object_epoch: result.object_epoch,
                    })
                    .collect();
                Ok(p::GetOperationStatusResponse {
                    state: state as i32,
                    owner: owner as i32,
                    subject_id: Some(subject.to_string()),
                    recorded_at: Some(timestamp(row.try_get("created_at").map_err(db)?)),
                    committed_at: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("committed_at")
                        .map_err(db)?
                        .map(timestamp),
                    results,
                    ..Default::default()
                })
            }
            Identity::SubjectCreate(identity) => {
                let subject = id(&identity.subject_id)?;
                let operation = id(&identity.operation_id)?;
                let row = sqlx::query("SELECT adopted_at FROM subject_seed_adoptions WHERE subject_id=$1 AND operation_id=$2")
                    .bind(subject)
                    .bind(operation)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(db)?;
                let Some(row) = row else {
                    return Ok(p::GetOperationStatusResponse {
                        state: p::OperationState::Unknown as i32,
                        owner: p::ErrorOwner::Subject as i32,
                        subject_id: Some(subject.to_string()),
                        ..Default::default()
                    });
                };
                Ok(p::GetOperationStatusResponse {
                    state: p::OperationState::Committed as i32,
                    owner: p::ErrorOwner::Subject as i32,
                    subject_id: Some(subject.to_string()),
                    recorded_at: Some(timestamp(row.try_get("adopted_at").map_err(db)?)),
                    results: vec![p::OperationResult {
                        reference: Some(to_ref(CognitiveRef::Subject(nous_core::SubjectId(
                            subject,
                        )))),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            }
            Identity::ConfigurationMutation(identity) => {
                let operation = id(&identity.operation_id)?;
                let row = sqlx::query("SELECT subject_id,created_at FROM configuration_mutation_receipts WHERE operation_id=$1")
                    .bind(operation)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(db)?;
                let Some(row) = row else {
                    return Ok(p::GetOperationStatusResponse {
                        state: p::OperationState::Unknown as i32,
                        owner: p::ErrorOwner::Configuration as i32,
                        ..Default::default()
                    });
                };
                let subject: Option<uuid::Uuid> = row.try_get("subject_id").map_err(db)?;
                Ok(p::GetOperationStatusResponse {
                    state: p::OperationState::Committed as i32,
                    owner: p::ErrorOwner::Configuration as i32,
                    subject_id: subject.map(|value| value.to_string()),
                    recorded_at: Some(timestamp(row.try_get("created_at").map_err(db)?)),
                    ..Default::default()
                })
            }
            Identity::ObservationRequest(identity) => {
                let subject = id(&identity.subject_id)?;
                let request = id(&identity.request_id)?;
                let row = sqlx::query("SELECT occurrence_id,created_at FROM observation_request_bindings WHERE subject_id=$1 AND request_id=$2")
                    .bind(subject)
                    .bind(request)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(db)?;
                let Some(row) = row else {
                    return Ok(p::GetOperationStatusResponse {
                        state: p::OperationState::Unknown as i32,
                        owner: p::ErrorOwner::Observation as i32,
                        subject_id: Some(subject.to_string()),
                        ..Default::default()
                    });
                };
                let occurrence: uuid::Uuid = row.try_get("occurrence_id").map_err(db)?;
                Ok(p::GetOperationStatusResponse {
                    state: p::OperationState::Committed as i32,
                    owner: p::ErrorOwner::Observation as i32,
                    subject_id: Some(subject.to_string()),
                    recorded_at: Some(timestamp(row.try_get("created_at").map_err(db)?)),
                    results: vec![p::OperationResult {
                        reference: Some(to_ref(CognitiveRef::Occurrence(nous_core::OccurrenceId(
                            occurrence,
                        )))),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
            }
            Identity::ModelWorkflow(identity) => {
                let subject = id(&identity.subject_id)?;
                let owner = workflow_owner(&identity.owner).ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Kernel,
                        nous_core::ErrorCode::InvalidRequest,
                        "model workflow owner is not registered",
                    )
                })?;
                if identity.operation_key.is_empty() || identity.operation_key.chars().count() > 256
                {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Kernel,
                        nous_core::ErrorCode::InvalidRequest,
                        "invalid model workflow operation_key",
                    ));
                }
                let row = sqlx::query("SELECT (outcome IS NOT NULL) AS completed,created_at,updated_at,lease_until FROM model_workflow_operations WHERE subject_id=$1 AND owner=$2 AND operation_key=$3")
                    .bind(subject)
                    .bind(&identity.owner)
                    .bind(&identity.operation_key)
                    .fetch_optional(self.0.store.pool())
                    .await
                    .map_err(db)?;
                let Some(row) = row else {
                    return Ok(p::GetOperationStatusResponse {
                        state: p::OperationState::Unknown as i32,
                        owner: owner as i32,
                        subject_id: Some(subject.to_string()),
                        ..Default::default()
                    });
                };
                let completed: bool = row.try_get("completed").map_err(db)?;
                Ok(p::GetOperationStatusResponse {
                    state: if completed {
                        p::OperationState::Completed as i32
                    } else {
                        p::OperationState::InProgress as i32
                    },
                    owner: owner as i32,
                    subject_id: Some(subject.to_string()),
                    recorded_at: Some(timestamp(row.try_get("created_at").map_err(db)?)),
                    updated_at: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("updated_at")
                        .map_err(db)?
                        .map(timestamp),
                    lease_until: row
                        .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("lease_until")
                        .map_err(db)?
                        .map(timestamp),
                    ..Default::default()
                })
            }
        }
    }
}
