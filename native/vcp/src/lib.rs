//! Native VCP kernels with typed, immutable input and output.

pub mod artifact;
pub mod assets;
pub mod baseline;
pub mod geometry;
pub mod graph;
pub mod pipeline;
pub mod projection;
pub mod river;
pub mod sensing;
pub mod thinking;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => message.fmt(f),
        }
    }
}
impl std::error::Error for Error {}

/// Cooperative cancellation is checked at numerical iteration/batch boundaries.
pub type CancellationCheck<'a> = Option<&'a (dyn Fn() -> bool + Sync)>;
pub fn check_cancelled(cancelled: CancellationCheck<'_>) -> std::result::Result<(), String> {
    if cancelled.is_some_and(|check| check()) {
        Err("CANCELLED: native computation cancelled".into())
    } else {
        Ok(())
    }
}
