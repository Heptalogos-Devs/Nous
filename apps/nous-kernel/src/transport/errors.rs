// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, ErrorCode, ErrorOwner};
use nous_protocol::{google::rpc, public as p};
use prost::Message;
use tonic::{Code, Status};
use uuid::Uuid;

/// Convert every owner error to exactly one typed wire detail at the transport boundary.
pub fn status(error: Error) -> Status {
    let (transport, recovery) = status_and_recovery(error.code);
    let trace_id = Uuid::now_v7().to_string();
    let detail = p::ErrorDetail {
        code: wire_code(error.code).into(),
        recovery: recovery.into(),
        context: error.context.into_iter().collect(),
        owner: wire_owner(error.owner).into(),
        trace_id: trace_id.clone(),
    };
    if error.code == ErrorCode::InternalFailure {
        tracing::error!(
            owner = ?error.owner,
            code = ?error.code,
            trace_id = %trace_id,
            source = ?error.source,
            diagnostic = %error.message,
            "kernel operation failed"
        );
    } else {
        tracing::debug!(
            owner = ?error.owner,
            code = ?error.code,
            trace_id = %trace_id,
            source = ?error.source,
            diagnostic = %error.message,
            "kernel error response"
        );
    }
    let message = error.message;
    let status = rpc::Status {
        code: transport as i32,
        message: message.clone(),
        details: vec![prost_types::Any {
            type_url: "type.googleapis.com/nous.wave.v1alpha1.ErrorDetail".into(),
            value: detail.encode_to_vec(),
        }],
    };
    Status::with_details(transport, message, status.encode_to_vec().into())
}

fn status_and_recovery(code: ErrorCode) -> (Code, p::ErrorRecovery) {
    use p::ErrorRecovery as Recovery;
    match code {
        ErrorCode::UnknownReference => (Code::NotFound, Recovery::ResolveReference),
        ErrorCode::AmbiguousReference => (Code::InvalidArgument, Recovery::SelectCandidate),
        ErrorCode::ReferenceTypeMismatch => (Code::InvalidArgument, Recovery::CorrectRequest),
        ErrorCode::ReferenceTombstoned => (Code::FailedPrecondition, Recovery::RediscoverReference),
        ErrorCode::UnresolvedMachinePlaceholder => {
            (Code::InvalidArgument, Recovery::ResolveReference)
        }
        ErrorCode::StaleContext => (Code::FailedPrecondition, Recovery::RefreshState),
        ErrorCode::StaleRevision => (Code::Aborted, Recovery::RefreshState),
        ErrorCode::OperationIdConflict => (Code::Aborted, Recovery::NewOperation),
        ErrorCode::OperationInProgress | ErrorCode::LeaseLost => {
            (Code::Aborted, Recovery::RetryOperation)
        }
        ErrorCode::CapabilityUnavailable => (Code::Unavailable, Recovery::CheckConfiguration),
        ErrorCode::InvalidRequest => (Code::InvalidArgument, Recovery::CorrectRequest),
        ErrorCode::NotFound => (Code::NotFound, Recovery::Unspecified),
        ErrorCode::Conflict => (Code::Aborted, Recovery::Unspecified),
        ErrorCode::FailedPrecondition => (Code::FailedPrecondition, Recovery::Unspecified),
        ErrorCode::Unavailable => (Code::Unavailable, Recovery::Unspecified),
        ErrorCode::InternalFailure => (Code::Internal, Recovery::Unspecified),
        ErrorCode::CapacityExhausted => (Code::ResourceExhausted, Recovery::RefreshState),
        ErrorCode::InstanceAlreadyRunning => (Code::AlreadyExists, Recovery::CheckConfiguration),
    }
}

fn wire_code(code: ErrorCode) -> p::ErrorCode {
    use ErrorCode as C;
    use p::ErrorCode as W;
    match code {
        C::UnknownReference => W::UnknownReference,
        C::AmbiguousReference => W::AmbiguousReference,
        C::ReferenceTypeMismatch => W::ReferenceTypeMismatch,
        C::ReferenceTombstoned => W::ReferenceTombstoned,
        C::UnresolvedMachinePlaceholder => W::UnresolvedMachinePlaceholder,
        C::StaleContext => W::StaleContext,
        C::StaleRevision => W::StaleRevision,
        C::OperationIdConflict => W::OperationIdConflict,
        C::OperationInProgress => W::OperationInProgress,
        C::LeaseLost => W::LeaseLost,
        C::CapabilityUnavailable => W::CapabilityUnavailable,
        C::InvalidRequest => W::InvalidRequest,
        C::NotFound => W::NotFound,
        C::Conflict => W::Conflict,
        C::FailedPrecondition => W::FailedPrecondition,
        C::Unavailable => W::Unavailable,
        C::InternalFailure => W::InternalFailure,
        C::CapacityExhausted => W::CapacityExhausted,
        C::InstanceAlreadyRunning => W::InstanceAlreadyRunning,
    }
}

fn wire_owner(owner: ErrorOwner) -> p::ErrorOwner {
    use ErrorOwner as O;
    use p::ErrorOwner as W;
    match owner {
        O::Identity => W::Identity,
        O::Subject => W::Subject,
        O::Memory => W::Memory,
        O::Material => W::Material,
        O::Runtime => W::Runtime,
        O::Configuration => W::Configuration,
        O::Concept => W::Concept,
        O::Observation => W::Observation,
        O::Resource => W::Resource,
        O::Kernel => W::Kernel,
        O::Core => W::Core,
        O::Persistence => W::Persistence,
        O::Instance => W::Instance,
        O::Cli => W::Cli,
    }
}
