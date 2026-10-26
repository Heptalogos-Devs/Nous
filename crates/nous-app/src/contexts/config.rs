use super::*;
use nous_foundation::configuration::{
    ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const CONTEXTS: ConfigKey<ContextsPolicy> = ConfigKey::new("contexts");
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ContextsPolicy {
    pub consumers: BTreeMap<String, ContextConsumerPolicy>,
    pub resident_limit: usize,
}
impl Default for ContextsPolicy {
    fn default() -> Self {
        Self {
            consumers: BTreeMap::from([("default".into(), ContextConsumerPolicy::default())]),
            resident_limit: 256,
        }
    }
}
impl ContextsPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.consumers.is_empty()
            || self.consumers.len() > 128
            || self.resident_limit == 0
            || self.resident_limit > 65536
        {
            return Err(Error::invalid(
                "context consumers must contain 1..128 named policies",
            ));
        }
        for (id, policy) in &self.consumers {
            validate_name(id, "context consumer")?;
            policy.validate()?;
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        CONTEXTS,
        ConfigDefinition {
            owner: "contexts".into(),
            description: "Consumer contribution limits, source kinds and projection stewardship."
                .into(),
            default: ContextsPolicy::default(),
            scopes: vec![
                ConfigLevel::Instance,
                ConfigLevel::Namespace,
                ConfigLevel::Subject,
            ],
            apply_mode: ConfigApplyMode::Live,
            write_policy: ConfigWritePolicy::Scoped,
        },
        ContextsPolicy::validate,
    )
}
impl App {
    pub(super) async fn context_policy(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
        consumer: &str,
    ) -> Result<(ContextConsumerPolicy, String)> {
        let snapshot = self
            .configuration
            .snapshot(
                &ConfigScope::Subject {
                    namespace_id: ns.clone(),
                    subject_id: subject.clone(),
                },
                false,
            )
            .await?;
        let policy = snapshot
            .get(CONTEXTS)?
            .consumers
            .get(consumer)
            .cloned()
            .ok_or_else(|| {
                Error::new(ErrorCode::Forbidden, "context consumer is not configured")
            })?;
        let digest = super::compile::hash(&json!({"consumer":consumer,"policy":policy}))?;
        Ok((policy, digest))
    }
    pub(super) async fn context_resident_limit(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<usize> {
        self.configuration
            .snapshot(
                &ConfigScope::Subject {
                    namespace_id: ns.clone(),
                    subject_id: subject.clone(),
                },
                false,
            )
            .await?
            .get(CONTEXTS)
            .map(|policy| policy.resident_limit)
    }
}
