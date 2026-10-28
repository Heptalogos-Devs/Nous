//! Subject-package snapshots, installation ownership and atomic withdrawal.
mod authorization;
mod install;
mod install_files;
mod install_units;
pub(crate) mod placement;
pub(crate) mod registry;
mod remap;
mod selection;
mod selection_graph;
mod selection_rows;
mod selection_tasks;
mod selection_units;
pub mod semantics;
mod snapshot;
mod snapshot_access;
mod vectors;
pub use registry::BundleWithdrawal;
