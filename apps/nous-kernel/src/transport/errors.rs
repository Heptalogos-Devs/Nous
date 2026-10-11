// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{DomainError, DomainErrorCode as DomainCode, Error};
use nous_protocol::{google::rpc, public as p};
use prost::Message;
use tonic::{Code, Status};

pub fn status(error: Error) -> Status {
    match error {
        Error::Domain(error) => domain_status(error),
        Error::Invalid(message) => Status::invalid_argument(message),
        Error::NotFound(message) => Status::not_found(message),
        Error::Conflict(message) => Status::aborted(message),
        Error::FailedPrecondition(message) => Status::failed_precondition(message),
        Error::Unavailable(message) => Status::unavailable(message),
        Error::Internal(message) | Error::Infrastructure(message) => {
            tracing::error!(%message, "kernel operation failed");
            Status::internal("Kernel operation failed")
        }
    }
}

fn domain_status(error: DomainError) -> Status {
    use p::{DomainErrorCode as WireCode, ErrorRecovery as Recovery};
    let (transport, code, recovery) = match error.code {
        DomainCode::UnknownReference => (
            Code::NotFound,
            WireCode::UnknownReference,
            Recovery::ResolveReference,
        ),
        DomainCode::AmbiguousReference => (
            Code::InvalidArgument,
            WireCode::AmbiguousReference,
            Recovery::SelectCandidate,
        ),
        DomainCode::ReferenceTypeMismatch => (
            Code::InvalidArgument,
            WireCode::ReferenceTypeMismatch,
            Recovery::CorrectRequest,
        ),
        DomainCode::ReferenceTombstoned => (
            Code::FailedPrecondition,
            WireCode::ReferenceTombstoned,
            Recovery::RediscoverReference,
        ),
        DomainCode::UnresolvedMachinePlaceholder => (
            Code::InvalidArgument,
            WireCode::UnresolvedMachinePlaceholder,
            Recovery::ResolveReference,
        ),
        DomainCode::StaleContext => (
            Code::FailedPrecondition,
            WireCode::StaleContext,
            Recovery::RefreshState,
        ),
        DomainCode::StaleRevision => (
            Code::Aborted,
            WireCode::StaleRevision,
            Recovery::RefreshState,
        ),
        DomainCode::OperationIdConflict => (
            Code::Aborted,
            WireCode::OperationIdConflict,
            Recovery::NewOperation,
        ),
        DomainCode::OperationInProgress => (
            Code::Aborted,
            WireCode::OperationInProgress,
            Recovery::RetryOperation,
        ),
        DomainCode::LeaseLost => (Code::Aborted, WireCode::LeaseLost, Recovery::RetryOperation),
        DomainCode::CapabilityUnavailable => (
            Code::Unavailable,
            WireCode::CapabilityUnavailable,
            Recovery::CheckConfiguration,
        ),
    };
    let detail = p::ErrorDetail {
        code: code.into(),
        recovery: recovery.into(),
        context: error.context.into_iter().collect(),
    };
    let status = rpc::Status {
        code: transport as i32,
        message: error.message.clone(),
        details: vec![prost_types::Any {
            type_url: "type.googleapis.com/nous.wave.v1alpha1.ErrorDetail".into(),
            value: detail.encode_to_vec(),
        }],
    };
    Status::with_details(transport, error.message, status.encode_to_vec().into())
}
