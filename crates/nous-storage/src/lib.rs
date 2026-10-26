//! Transactional SQLite authority and durable work.
mod authority;
mod codec;
mod cognition;
mod configuration;
mod context;
mod identity;
mod media;
mod model;
mod prompt;
mod source;
mod vector;
pub use authority::*;
pub use cognition::*;
pub use model::*;
use nous_domain::{Error, ErrorCode};
pub use source::*;
pub use vector::*;
fn not_found() -> Error {
    Error::new(ErrorCode::NotFound, "object was not found")
}

fn storage_error(error: impl std::error::Error + 'static) -> Error {
    Error::new(ErrorCode::Internal, "storage operation failed")
        .with_cause(&error)
        .with_context("component", "storage")
}

fn write_error(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(ref database) = error {
        if database.is_unique_violation() {
            return Error::new(ErrorCode::Conflict, "object already exists").with_cause(&error);
        }
        if database.is_foreign_key_violation() || database.is_check_violation() {
            return Error::new(
                ErrorCode::InvalidArgument,
                "object scope or field violates the storage contract",
            )
            .with_cause(&error);
        }
    }
    storage_error(error)
}
