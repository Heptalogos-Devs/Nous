//! Managed paths, bootstrap configuration, protected credentials and runtime ownership.
mod config;
mod lock;
mod paths;
pub use config::{InstanceConfig, ServerConfig, initialize};
pub use lock::InstanceGuard;
pub(crate) use lock::lock_contended;
pub use paths::InstancePaths;
