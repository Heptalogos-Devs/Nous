//! Infrastructure execution budgets owned by Cognitive Runtime.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_configuration::*;
use nous_core::{Error, Result};
pub const QUERY_SLOTS: ConfigKey<usize> = ConfigKey::new("runtime.query_lease_slots");
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        QUERY_SLOTS,
        "cognitive-runtime",
        "Concurrent retained query slots.",
        16,
        ConfigExposure::Developer,
        ConfigScopePolicy::SystemOnly,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |value| {
            if (1..=64).contains(value) {
                Ok(())
            } else {
                Err(Error::Invalid("query lease slots must be 1..64".into()))
            }
        },
    )?;
    registry.describe(QUERY_SLOTS.path(), |d| {
        d.unit = Some("items".into());
        d.json_schema["minimum"] = serde_json::json!(1);
        d.json_schema["maximum"] = serde_json::json!(64);
    })
}
