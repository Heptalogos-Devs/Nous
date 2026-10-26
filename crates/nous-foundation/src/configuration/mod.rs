//! Typed configuration, scope inheritance and owner-controlled activation.
mod changes;
mod core;
mod registry;
mod resolution;
mod service;
mod snapshot;

pub use changes::{ConfigActivation, ConfigEdit};
pub use core::{AUTH, NETWORK, SERVER, install_bootstrap_seed_in, register_core};
pub use registry::{
    ConfigDefinition, ConfigDescriptor, ConfigKey, ConfigRegistry, ConfigRegistryBuilder,
    ConfigWritePolicy,
};
pub use service::{ConfigChange, ConfigChangeSet, Configuration, ConfigurationView};
pub use snapshot::{ConfigSnapshot, ConfigSource, ResolvedConfigValue};
