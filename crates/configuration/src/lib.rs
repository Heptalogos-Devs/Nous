//! Shared configuration registration, resolution, persistence, and snapshots.
//!
//! This crate owns configuration mechanics only. Domain meaning stays with the
//! registering owner, which resolves a snapshot into its typed policy.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod identity;
mod key;
mod profile;
pub use profile::ReferenceProfile;
mod registry;
mod service;
mod snapshot;

pub use key::*;
pub use registry::{
    CONFIG_SCHEMA_DIALECT, ConfigDescriptor, ConfigRegistry, ConfigRegistryBuilder,
    ConfigurationBootstrapBundle,
};
pub use service::{ConfigChangeOutcome, ConfigurationService};
pub use snapshot::{ConfigSnapshot, ResolvedConfigValue};

use nous_core::Result;

pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    let bool_validator = |_: &bool| Ok(());
    registry.register(
        PROCESS_MEMORY,
        "configuration-service",
        "Whether the process provides Memory cognition.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        bool_validator,
    )?;
    registry.register(
        SUBJECT_DEFAULT_MEMORY,
        "configuration-service",
        "Default Memory capability for newly created Subjects.",
        true,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::NewSubjectsOnly,
        ConfigSemanticEffect::SubjectProvisioning,
        bool_validator,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TEST_KEY: ConfigKey<f64> = ConfigKey::new("test.policy.threshold");

    #[test]
    fn duplicate_keys_and_invalid_defaults_are_rejected() {
        let mut builder = ConfigRegistryBuilder::new();
        builder
            .register(
                TEST_KEY,
                "test",
                "threshold",
                1.0,
                ConfigExposure::Advanced,
                ConfigScopePolicy::SubjectOverrideAllowed,
                ConfigApplyMode::Live,
                ConfigSemanticEffect::QueryPolicy,
                |value| {
                    if *value >= 0.0 {
                        Ok(())
                    } else {
                        Err(nous_core::Error::Invalid("negative".into()))
                    }
                },
            )
            .unwrap();
        assert!(
            builder
                .register(
                    TEST_KEY,
                    "test",
                    "threshold",
                    1.0,
                    ConfigExposure::Advanced,
                    ConfigScopePolicy::SubjectOverrideAllowed,
                    ConfigApplyMode::Live,
                    ConfigSemanticEffect::QueryPolicy,
                    |_| Ok(()),
                )
                .is_err()
        );
    }

    #[test]
    fn deployment_is_descriptor_aware_and_snapshot_digest_is_subset_scoped() {
        let mut builder = ConfigRegistryBuilder::new();
        builder
            .register(
                TEST_KEY,
                "test",
                "threshold",
                1.0,
                ConfigExposure::Advanced,
                ConfigScopePolicy::SubjectOverrideAllowed,
                ConfigApplyMode::Live,
                ConfigSemanticEffect::QueryPolicy,
                |_| Ok(()),
            )
            .unwrap();
        let registry = builder.finish().unwrap();
        let values = std::collections::BTreeMap::from([(
            TEST_KEY.path().to_owned(),
            ResolvedConfigValue {
                json: json!(1.0),
                source: ConfigSource::ReferenceDefault,
            },
        )]);
        let snapshot = ConfigSnapshot::new(None, 1, registry.clone(), values).unwrap();
        let first = snapshot.digest_for(&[TEST_KEY.path()]).unwrap();
        assert_eq!(snapshot.get(TEST_KEY).unwrap(), 1.0);
        assert_eq!(first, snapshot.digest_for(&[TEST_KEY.path()]).unwrap());
        assert_eq!(
            registry
                .deployment_values(&json!({"test":{"policy":{"threshold":2.0}}}))
                .unwrap()[TEST_KEY.path()],
            json!(2.0)
        );
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;
    use serde_json::json;

    fn structured_descriptor() -> ConfigDescriptor {
        ConfigDescriptor {
            path: "gateway_profiles".into(),
            owner: "core-model".into(),
            title: "Gateways".into(),
            description: "Gateway profile map".into(),
            category: "models".into(),
            json_schema: json!({"$schema": CONFIG_SCHEMA_DIALECT,"type":"object","additionalProperties":{"type":"object","properties":{"enabled":{"type":"boolean"}},"required":["enabled"],"additionalProperties":false}}),
            reference_default: json!({}),
            exposure: ConfigExposure::Standard,
            scope_policy: ConfigScopePolicy::SystemOnly,
            storage_policy: ConfigStoragePolicy::Overrideable,
            apply_mode: ConfigApplyMode::RestartProcess,
            semantic_effect: ConfigSemanticEffect::Operational,
            unit: None,
            sensitivity: ConfigSensitivity::CredentialReference,
            reference_profile: None,
        }
    }

    #[test]
    fn imported_structures_consume_whole_values_and_reject_unknown_paths() {
        let mut builder = ConfigRegistryBuilder::new();
        builder.import(structured_descriptor()).unwrap();
        let registry = builder.finish().unwrap();
        let document = json!({"gateway_profiles":{"local":{"enabled":true}}});
        assert_eq!(
            registry.deployment_values(&document).unwrap()["gateway_profiles"],
            document["gateway_profiles"]
        );
        assert!(
            registry
                .deployment_values(&json!({"gateway_profiles":{"local":{"enabled":"yes"}}}))
                .is_err()
        );
        assert!(registry.deployment_values(&json!({"unknown":{}})).is_err());
        assert!(registry.deployment_values(&json!({"settings":{}})).is_err());
    }

    #[test]
    fn catalog_validates_dialect_schema_defaults_and_overlap() {
        let descriptor = structured_descriptor();
        let mut builder = ConfigRegistryBuilder::new();
        builder.import(descriptor.clone()).unwrap();
        let mut child = descriptor.clone();
        child.path = "gateway_profiles.enabled".into();
        assert!(builder.import(child).is_err());
        let mut invalid = descriptor.clone();
        invalid.json_schema["type"] = json!("nonsense");
        assert!(ConfigRegistryBuilder::new().import(invalid).is_err());
        let mut invalid = descriptor.clone();
        invalid.json_schema["$schema"] = json!("https://json-schema.org/draft-07/schema");
        assert!(ConfigRegistryBuilder::new().import(invalid).is_err());
        let mut invalid = descriptor;
        invalid.reference_default = json!(42);
        let mut builder = ConfigRegistryBuilder::new();
        builder.import(invalid).unwrap();
        assert!(builder.finish().is_err());
    }

    #[test]
    fn semantic_catalog_digest_excludes_prose_and_dynamic_paths_own_their_text() {
        let mut a = ConfigRegistryBuilder::new();
        a.import(structured_descriptor()).unwrap();
        let a = a.finish().unwrap();
        let mut descriptor = structured_descriptor();
        descriptor.title = "New title".into();
        descriptor.description = "New prose".into();
        descriptor.exposure = ConfigExposure::Developer;
        let mut b = ConfigRegistryBuilder::new();
        b.import(descriptor.clone()).unwrap();
        let b = b.finish().unwrap();
        assert_eq!(a.semantic_digest(), b.semantic_digest());
        assert_ne!(a.catalog_digest(), b.catalog_digest());
        assert_eq!(
            a.reference_snapshot().unwrap().effective_digest,
            b.reference_snapshot().unwrap().effective_digest
        );
        descriptor.json_schema["maxProperties"] = json!(8);
        let mut c = ConfigRegistryBuilder::new();
        c.import(descriptor).unwrap();
        assert_ne!(a.semantic_digest(), c.finish().unwrap().semantic_digest());
        let path: ConfigPath = "dynamic.policy".to_owned().parse().unwrap();
        assert_eq!(&*path.0, "dynamic.policy");
    }

    #[test]
    fn subset_identity_uses_a_set_of_keys() {
        let mut builder = ConfigRegistryBuilder::new();
        register_configuration(&mut builder).unwrap();
        let snapshot = builder.finish().unwrap().reference_snapshot().unwrap();
        assert_eq!(
            snapshot
                .digest_for(&[PROCESS_MEMORY.path(), SUBJECT_DEFAULT_MEMORY.path()])
                .unwrap(),
            snapshot
                .digest_for(&[
                    SUBJECT_DEFAULT_MEMORY.path(),
                    PROCESS_MEMORY.path(),
                    PROCESS_MEMORY.path()
                ])
                .unwrap(),
        );
    }

    #[test]
    fn structured_policy_is_normalized_by_its_owner_before_freezing() {
        #[derive(Clone, Copy, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct Policy {
            enabled: bool,
            #[serde(default)]
            limit: u32,
        }
        let key: ConfigKey<Policy> = ConfigKey::new("test.policy");
        let mut builder = ConfigRegistryBuilder::new();
        builder
            .register(
                key,
                "test",
                "policy",
                Policy {
                    enabled: true,
                    limit: 0,
                },
                ConfigExposure::Advanced,
                ConfigScopePolicy::SubjectOverrideAllowed,
                ConfigApplyMode::Live,
                ConfigSemanticEffect::QueryPolicy,
                |_| Ok(()),
            )
            .unwrap();
        let registry = builder.finish().unwrap();
        let snapshot = |json| {
            ConfigSnapshot::new(
                None,
                1,
                registry.clone(),
                std::collections::BTreeMap::from([(
                    key.path().to_owned(),
                    ResolvedConfigValue {
                        json,
                        source: ConfigSource::DeploymentFile,
                    },
                )]),
            )
            .unwrap()
        };
        let omitted = snapshot(json!({"enabled":true}));
        let explicit = snapshot(json!({"enabled":true,"limit":0}));
        assert_eq!(omitted.json(key.path()), explicit.json(key.path()));
        assert_eq!(omitted.effective_digest, explicit.effective_digest);
        assert_eq!(omitted.get(key).unwrap().limit, 0);
    }
}
