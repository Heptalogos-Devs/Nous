//! Material and evidence semantic owner plus its persistence/service operations.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod domain;
pub use domain::*;

pub mod service;
pub use service::*;

pub const MAX_UPLOAD_BYTES: nous_configuration::ConfigKey<u64> =
    nous_configuration::ConfigKey::new("object_store.max_upload_bytes");
pub const DESCRIPTION_SEGMENT_BYTES: nous_configuration::ConfigKey<usize> =
    nous_configuration::ConfigKey::new("material.description_segment_bytes");
pub const ABSOLUTE_UPLOAD_BYTES: u64 = 9_007_199_254_740_991;
pub fn register_configuration(
    registry: &mut nous_configuration::ConfigRegistryBuilder,
) -> nous_core::Result<()> {
    use nous_configuration::*;
    registry.register(
        DESCRIPTION_SEGMENT_BYTES,
        "material",
        "Target UTF-8 bytes per description segment.",
        2048,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::AuthorityFormation,
        |value| {
            if (4..=65536).contains(value) {
                Ok(())
            } else {
                Err(nous_core::Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "description segment bytes must be 4..65536",
                ))
            }
        },
    )?;
    registry.describe(DESCRIPTION_SEGMENT_BYTES.path(), |d| {
        d.unit = Some("bytes".into());
        d.json_schema["minimum"] = serde_json::json!(4);
        d.json_schema["maximum"] = serde_json::json!(65536);
    })?;
    registry.register(
        MAX_UPLOAD_BYTES,
        "material",
        "Artifact upload byte budget.",
        8 * 1024 * 1024 * 1024,
        ConfigExposure::Advanced,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::RestartProcess,
        ConfigSemanticEffect::Operational,
        |value| {
            if *value > 0 && *value <= ABSOLUTE_UPLOAD_BYTES {
                Ok(())
            } else {
                Err(nous_core::Error::new(
                    nous_core::ErrorOwner::Material,
                    nous_core::ErrorCode::InvalidRequest,
                    "upload budget exceeds bounds",
                ))
            }
        },
    )?;
    registry.describe(MAX_UPLOAD_BYTES.path(), |d| {
        d.unit = Some("bytes".into());
        d.json_schema["minimum"] = serde_json::json!(1);
        d.json_schema["maximum"] = serde_json::json!(ABSOLUTE_UPLOAD_BYTES);
    })
}
