// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

mod catalog {
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
                        json: registry.normalize(key.path(), &json).unwrap(),
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
