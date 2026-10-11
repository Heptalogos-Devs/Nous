// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{Degradation, ErrorCode, ErrorOwner, RequirementStrength};
use nous_protocol::google::rpc;
use nous_runtime::{BoundQuery, QueryHostPlan, QueryHostRole};
use prost::Message;
use tonic::{Code, Status};

pub(super) fn host_role(value: Option<k::QueryHostRole>) -> QueryHostRole {
    let value = value.unwrap_or(k::QueryHostRole {
        configured: false,
        requirement: k::QueryHostRequirement::Optional as i32,
    });
    QueryHostRole {
        configured: value.configured,
        requirement: match k::QueryHostRequirement::try_from(value.requirement).ok() {
            Some(k::QueryHostRequirement::Required) => RequirementStrength::Required,
            Some(k::QueryHostRequirement::Optional) | None => RequirementStrength::Optional,
            Some(k::QueryHostRequirement::Forbidden) => RequirementStrength::Forbidden,
            Some(k::QueryHostRequirement::Unspecified) => RequirementStrength::Optional,
        },
    }
}

pub(super) fn host_plan(value: Option<k::QueryHostPlan>) -> QueryHostPlan {
    let value = value.unwrap_or_default();
    QueryHostPlan {
        embedding: host_role(value.embedding),
        concept_enrichment: host_role(value.concept_enrichment),
        rerank: host_role(value.rerank),
        rerank_candidate_limit: value.rerank_candidate_limit as usize,
    }
}

pub(super) fn wire_requirement(value: RequirementStrength) -> i32 {
    match value {
        RequirementStrength::Required => k::QueryHostRequirement::Required as i32,
        RequirementStrength::Preferred => k::QueryHostRequirement::Optional as i32,
        RequirementStrength::Optional => k::QueryHostRequirement::Optional as i32,
        RequirementStrength::Forbidden => k::QueryHostRequirement::Forbidden as i32,
    }
}

pub(super) fn capability_error(capability: &'static str) -> nous_core::Error {
    nous_core::Error::new(
        ErrorOwner::Core,
        ErrorCode::CapabilityUnavailable,
        "A required query capability is unavailable",
    )
    .with_context("capability", capability)
}

pub(super) fn unavailable_embedding(reason: &'static str) -> nous_core::Error {
    nous_core::Error::new(
        ErrorOwner::Kernel,
        ErrorCode::CapabilityUnavailable,
        "Query embedding material is unavailable",
    )
    .with_context("capability", reason)
}

pub(super) fn rpc_status(value: rpc::Status) -> Status {
    let code = Code::from_i32(value.code);
    Status::with_details(code, value.message.clone(), value.encode_to_vec().into())
}

pub(super) fn add_degradation(bound: &mut BoundQuery, code: &str) {
    bound.activation.degradation.push(Degradation {
        code: code.into(),
        detail: None,
    });
}
