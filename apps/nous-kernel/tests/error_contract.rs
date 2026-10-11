// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, ErrorCode, ErrorOwner};
use nous_kernel::transport::status;
use nous_protocol::{google::rpc, public};
use prost::Message;
use tonic::Code;

#[test]
fn status_has_one_owner_detail_and_never_exposes_source() {
    let status = status(
        Error::new(
            ErrorOwner::Configuration,
            ErrorCode::InvalidRequest,
            "Invalid configuration value",
        )
        .with_context("path", "runtime.resident_limit")
        .with_source(std::io::Error::other("secret configuration payload")),
    );

    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(status.message(), "Invalid configuration value");
    assert!(!status.message().contains("secret configuration payload"));

    let encoded = rpc::Status::decode(status.details()).expect("google.rpc.Status");
    assert_eq!(encoded.message, "Invalid configuration value");
    assert_eq!(encoded.details.len(), 1);
    assert_eq!(
        encoded.details[0].type_url,
        "type.googleapis.com/nous.wave.v1alpha1.ErrorDetail"
    );
    let detail = public::ErrorDetail::decode(encoded.details[0].value.as_slice())
        .expect("typed ErrorDetail");
    assert_eq!(detail.code, public::ErrorCode::InvalidRequest as i32);
    assert_eq!(detail.owner, public::ErrorOwner::Configuration as i32);
    assert_eq!(
        detail.recovery,
        public::ErrorRecovery::CorrectRequest as i32
    );
    assert_eq!(
        detail.context.get("path").map(String::as_str),
        Some("runtime.resident_limit")
    );
    assert!(!detail.trace_id.is_empty());
}

#[test]
fn capacity_errors_use_the_resource_exhausted_transport_status() {
    let status = status(Error::new(
        ErrorOwner::Runtime,
        ErrorCode::CapacityExhausted,
        "Query capacity is exhausted",
    ));
    assert_eq!(status.code(), Code::ResourceExhausted);
    let encoded = rpc::Status::decode(status.details()).expect("google.rpc.Status");
    let detail = public::ErrorDetail::decode(encoded.details[0].value.as_slice())
        .expect("typed ErrorDetail");
    assert_eq!(detail.code, public::ErrorCode::CapacityExhausted as i32);
    assert_eq!(detail.owner, public::ErrorOwner::Runtime as i32);
    assert_eq!(detail.recovery, public::ErrorRecovery::RefreshState as i32);
}
