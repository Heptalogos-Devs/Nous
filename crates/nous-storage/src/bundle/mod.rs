//! Subject-package snapshots, installation ownership and atomic withdrawal.
mod authorization;
mod install;
pub(crate) mod placement;
pub(crate) mod registry;
mod remap;
mod snapshot;
mod vectors;
pub use registry::BundleWithdrawal;
