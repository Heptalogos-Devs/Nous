//! Authority-frozen discovery and shared cognitive embedding work.
mod embedding;
pub mod embedding_model;
mod embedding_worker;
mod query;
mod semantic;
mod view;

pub(crate) use embedding::provenance;

use crate::{App, operation_key};
use nous_domain::*;
use nous_storage::{BeginOperation, OperationReceipt};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn engine_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        ErrorCode::DependencyUnavailable,
        format!("retrieval engine: {error}"),
    )
}
