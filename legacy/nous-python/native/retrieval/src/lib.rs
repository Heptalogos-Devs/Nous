//! Rebuildable lexical, vector and VCP asset engines.
mod grouping;
mod lexical;
mod ordered_dedup;
pub use lexical::*;
pub mod vector;

pub mod bundles;
pub mod query;
pub mod snapshot;
#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
    pub details: serde_json::Value,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(f)
    }
}
impl std::error::Error for Failure {}
pub fn failure(code: &'static str, message: impl ToString) -> anyhow::Error {
    Failure {
        code,
        message: message.to_string(),
        details: serde_json::json!({"reason":message.to_string()}),
    }
    .into()
}

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Default)]
pub struct CancellationToken {
    inner: Arc<AtomicBool>,
}
impl CancellationToken {
    pub fn check(&self) -> anyhow::Result<()> {
        if self.inner.load(Ordering::Relaxed) {
            Err(failure("wave.cancelled", "Native operation cancelled"))
        } else {
            Ok(())
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::Relaxed)
    }
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::Relaxed)
    }
}

pub mod context;

pub mod engine;
