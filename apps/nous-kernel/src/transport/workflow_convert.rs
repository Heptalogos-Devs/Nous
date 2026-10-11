// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{convert::*, k};
use nous_core::{Error, Result};

pub(super) fn lease(value: k::WorkflowLease) -> Result<nous_persistence::WorkflowLease> {
    Ok(nous_persistence::WorkflowLease {
        subject: nous_core::SubjectId(id(&value.subject_id)?),
        owner: nous_persistence::WorkflowOwner::new(&value.owner)?,
        operation_key: value.operation_key,
        token: id(&value.token)?,
    })
}
pub(super) fn lease_proto(value: nous_persistence::WorkflowLease) -> k::WorkflowLease {
    k::WorkflowLease {
        subject_id: value.subject.0.to_string(),
        owner: value.owner.as_str().into(),
        operation_key: value.operation_key,
        token: value.token.to_string(),
    }
}

pub(super) fn payload(value: k::WorkflowPayload) -> Result<nous_core::WorkflowPayload> {
    Ok(nous_core::WorkflowPayload {
        payload: super::model::workflow_json(&value.payload_json)?,
        dependencies: value
            .dependencies
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
        purged: value.purged,
    })
}
pub(super) fn payload_proto(value: nous_core::WorkflowPayload) -> k::WorkflowPayload {
    k::WorkflowPayload {
        payload_json: value.payload.to_string(),
        dependencies: value.dependencies.into_iter().map(to_ref).collect(),
        purged: value.purged,
    }
}
pub(super) fn snapshot(value: k::WorkflowSnapshot) -> Result<nous_core::WorkflowSnapshot> {
    Ok(nous_core::WorkflowSnapshot {
        content: payload(required(value.content, "workflow content")?)?,
        cognitive_formed_at: time(value.cognitive_formed_at)?,
        maintenance_claim: value
            .maintenance_claim
            .map(|claim| -> Result<nous_core::MaintenanceClaim> {
                Ok(nous_core::MaintenanceClaim {
                    need_id: id(&claim.need_id)?,
                    lease_token: id(&claim.lease_token)?,
                    trigger_authority_seq: claim.trigger_authority_seq,
                    trigger_revision: claim.trigger_revision,
                })
            })
            .transpose()?,
    })
}
pub(super) fn snapshot_proto(value: nous_core::WorkflowSnapshot) -> k::WorkflowSnapshot {
    k::WorkflowSnapshot {
        content: Some(payload_proto(value.content)),
        cognitive_formed_at: value.cognitive_formed_at.map(timestamp),
        maintenance_claim: value.maintenance_claim.map(|claim| k::MaintenanceClaim {
            need_id: claim.need_id.to_string(),
            lease_token: claim.lease_token.to_string(),
            trigger_authority_seq: claim.trigger_authority_seq,
            trigger_revision: claim.trigger_revision,
        }),
    }
}
fn count(value: Option<f64>) -> Result<Option<u64>> {
    value
        .map(|value| {
            if value.is_finite()
                && value.fract() == 0.0
                && (0.0..=9_007_199_254_740_991.0).contains(&value)
            {
                Ok(value as u64)
            } else {
                Err(Error::Invalid("invalid execution usage count".into()))
            }
        })
        .transpose()
}
pub(super) fn telemetry(value: k::ExecutionTelemetry) -> Result<nous_core::ExecutionTelemetry> {
    let result = nous_core::ExecutionTelemetry {
        attempts: value
            .attempts
            .into_iter()
            .map(|attempt| {
                Ok(nous_core::ExecutionAttempt {
                    execution_profile: attempt.execution_profile,
                    model_profile: attempt.model_profile,
                    status: enum_value(&attempt.status)?,
                    failure_class: attempt.failure_class,
                    latency_ms: attempt.latency_ms,
                    usage: attempt
                        .usage
                        .map(|usage| -> Result<nous_core::ExecutionUsage> {
                            Ok(nous_core::ExecutionUsage {
                                input_tokens: count(usage.input_tokens)?,
                                output_tokens: count(usage.output_tokens)?,
                                total_tokens: count(usage.total_tokens)?,
                            })
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_>>()?,
        successful_execution_profile: value.successful_execution_profile,
        omitted_attempts: value.omitted_attempts,
    };
    result.validate()?;
    Ok(result)
}
pub(super) fn telemetry_proto(value: nous_core::ExecutionTelemetry) -> k::ExecutionTelemetry {
    k::ExecutionTelemetry {
        attempts: value
            .attempts
            .into_iter()
            .map(|attempt| k::ExecutionAttempt {
                execution_profile: attempt.execution_profile,
                model_profile: attempt.model_profile,
                status: enum_name(attempt.status),
                failure_class: attempt.failure_class,
                latency_ms: attempt.latency_ms,
                usage: attempt.usage.map(|usage| k::ExecutionUsage {
                    input_tokens: usage.input_tokens.map(|v| v as f64),
                    output_tokens: usage.output_tokens.map(|v| v as f64),
                    total_tokens: usage.total_tokens.map(|v| v as f64),
                }),
            })
            .collect(),
        successful_execution_profile: value.successful_execution_profile,
        omitted_attempts: value.omitted_attempts,
    }
}
