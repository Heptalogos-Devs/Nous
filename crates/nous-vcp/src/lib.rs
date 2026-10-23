//! Native VCP kernels with typed, immutable input and output.
//! Current VCP adaptations retain CC BY-NC-SA 4.0; baseline modules retain Apache-2.0.

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
