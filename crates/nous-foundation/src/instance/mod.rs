//! Managed paths, bootstrap layout and sole-runtime ownership.
mod config;
mod lock;
mod paths;
pub use config::{InstanceConfig, ServerConfig, initialize, initialize_staging};
pub use lock::InstanceGuard;
pub(crate) use lock::lock_contended;
pub use paths::InstancePaths;
pub(crate) use paths::is_within;
