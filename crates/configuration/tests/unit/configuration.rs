// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

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
                    Err(nous_core::Error::new(
                        nous_core::ErrorOwner::Configuration,
                        nous_core::ErrorCode::InvalidRequest,
                        "negative",
                    ))
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
