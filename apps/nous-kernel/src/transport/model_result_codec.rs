// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::{convert::*, k};
use nous_core::{Error, Result};

pub(super) fn identity(
    value: k::OperationIdentity,
) -> Result<nous_persistence::ModelOperationIdentity> {
    super::model::private_result_owner(&value.owner)?;
    Ok(nous_persistence::ModelOperationIdentity {
        subject: nous_core::SubjectId(id(&value.subject_id)?),
        owner: value.owner,
        operation_key: value.operation_key,
        semantic_digest: value.semantic_digest,
    })
}

pub(super) fn record(value: nous_persistence::ModelResultRecord) -> k::OperationRecord {
    k::OperationRecord {
        context: value.context.map(context_proto),
        proposal: value.proposal.map(payload_proto),
        outcome: value.outcome.map(payload_proto),
        execution_telemetry: value.execution_telemetry.map(telemetry_proto),
    }
}

pub(super) fn payload(value: k::ResultPayload) -> Result<nous_core::ResultPayload> {
    Ok(nous_core::ResultPayload {
        payload: super::model::workflow_json(&value.payload_json)?,
        dependencies: value
            .dependencies
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
        purged: value.purged,
    })
}
pub(super) fn payload_proto(value: nous_core::ResultPayload) -> k::ResultPayload {
    k::ResultPayload {
        payload_json: value.payload.to_string(),
        dependencies: value.dependencies.into_iter().map(to_ref).collect(),
        purged: value.purged,
    }
}
pub(super) fn context(value: k::ModelResultContext) -> Result<nous_core::ModelResultContext> {
    Ok(nous_core::ModelResultContext {
        dependencies: value
            .dependencies
            .into_iter()
            .map(from_ref)
            .collect::<Result<_>>()?,
        cognitive_formed_at: value.cognitive_formed_at.map(domain_stamp).transpose()?,
        maintenance_trigger: value
            .maintenance_trigger
            .map(|claim| -> Result<nous_core::MaintenanceTrigger> {
                Ok(nous_core::MaintenanceTrigger {
                    need_id: id(&claim.need_id)?,
                    trigger_authority_seq: claim.trigger_authority_seq,
                    trigger_revision: claim.trigger_revision,
                })
            })
            .transpose()?,
    })
}
pub(super) fn context_proto(value: nous_core::ModelResultContext) -> k::ModelResultContext {
    k::ModelResultContext {
        dependencies: value.dependencies.into_iter().map(to_ref).collect(),
        cognitive_formed_at: value.cognitive_formed_at.as_ref().map(domain_stamp_proto),
        maintenance_trigger: value
            .maintenance_trigger
            .map(|claim| k::MaintenanceTrigger {
                need_id: claim.need_id.to_string(),
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
