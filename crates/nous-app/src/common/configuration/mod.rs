//! Typed configuration, scope inheritance and owner-controlled activation.
mod changes;
mod core;
mod management;
mod registry;
mod resolution;
mod service;
mod snapshot;

pub use changes::{ConfigActivation, ConfigEdit};
pub use core::{AUTH, NETWORK, SERVER, register_core, reset_restored_auth};
pub use management::{ConfigChangeRequest, ConfigRequest};
pub use registry::{
    ConfigDefinition, ConfigDescriptor, ConfigKey, ConfigRegistry, ConfigRegistryBuilder,
};
pub use service::{ConfigChange, ConfigChangeSet, Configuration, ConfigurationView};
pub use snapshot::{ConfigSnapshot, ConfigSource, ResolvedConfigValue};
