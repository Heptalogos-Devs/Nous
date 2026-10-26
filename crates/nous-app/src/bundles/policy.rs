use crate::bundle_format::Limits;
use nous_domain::*;
use nous_foundation::configuration::{
    ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy,
};
use serde::{Deserialize, Serialize};
pub const BUNDLES: ConfigKey<BundlePolicy> = ConfigKey::new("bundles");
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct BundlePolicy {
    pub limits: Limits,
}
impl BundlePolicy {
    pub fn validate(&self) -> Result<()> {
        let v = &self.limits;
        if v.archive_bytes == 0
            || v.archive_bytes > i64::MAX as u64
            || v.member_bytes == 0
            || v.expanded_bytes == 0
            || v.member_bytes > v.expanded_bytes
            || v.json_bytes == 0
            || v.json_bytes > v.expanded_bytes
            || !(1..=100000).contains(&v.members)
            || !(1..=10000000).contains(&v.json_nodes)
            || !(1..=128).contains(&v.json_depth)
            || v.central_directory_bytes == 0
        {
            return Err(Error::invalid(
                "bundle inventory, expansion and JSON limits are out of range",
            ));
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        BUNDLES,
        ConfigDefinition {
            owner: "bundles".into(),
            description: "Portable subject package archive and expansion limits.".into(),
            default: BundlePolicy::default(),
            scopes: vec![
                ConfigLevel::Instance,
                ConfigLevel::Namespace,
                ConfigLevel::Subject,
            ],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::Scoped,
        },
        BundlePolicy::validate,
    )
}
