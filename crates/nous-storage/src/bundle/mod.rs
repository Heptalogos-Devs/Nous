//! Subject-package snapshots, installation ownership and atomic withdrawal.
mod authorization;
mod install;
pub(crate) mod placement;
pub(crate) mod registry;
mod remap;
mod selection;
mod selection_graph;
mod selection_rows;
pub mod semantics;
mod snapshot;
mod snapshot_access;
mod vectors;
pub use registry::BundleWithdrawal;
