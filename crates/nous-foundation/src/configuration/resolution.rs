//! Whole-key inheritance and complete owner validation for a selected scope.
use super::{ConfigSnapshot, ConfigSource, Configuration, ResolvedConfigValue};
use nous_domain::{ConfigEntry, ConfigScope, Error, Result};
use std::collections::BTreeMap;

impl Configuration {
    pub(super) fn resolve(
        &self,
        scope: &ConfigScope,
        revision: i64,
        entries: &[ConfigEntry],
        desired: bool,
    ) -> Result<ConfigSnapshot> {
        let mut values = self
            .registry
            .descriptors()
            .map(|definition| {
                let value = match self.deployment.get(&definition.key) {
                    Some(value) => ResolvedConfigValue {
                        json: value.clone(),
                        source: ConfigSource::Deployment,
                    },
                    None => ResolvedConfigValue {
                        json: definition.default.clone(),
                        source: ConfigSource::Builtin,
                    },
                };
                (definition.key.clone(), value)
            })
            .collect::<BTreeMap<_, _>>();
        for layer in hierarchy(scope) {
            for entry in entries.iter().filter(|entry| entry.scope == layer) {
                let definition = self.registry.descriptor(&entry.key)?;
                if definition.apply_mode != entry.apply_mode
                    || !definition.scopes.contains(&entry.scope.level())
                {
                    return Err(Error::invalid(
                        "persisted configuration does not match its owner definition",
                    ));
                }
                if let Some(value) = if desired {
                    &entry.desired
                } else {
                    &entry.active
                } {
                    values.insert(
                        entry.key.clone(),
                        ResolvedConfigValue {
                            json: self.registry.normalize(&entry.key, value.clone())?,
                            source: ConfigSource::Override {
                                scope: entry.scope.clone(),
                                revision: entry.revision,
                            },
                        },
                    );
                }
            }
        }
        let snapshot = ConfigSnapshot::new(scope.clone(), revision, desired, values)?;
        self.registry.validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }
}

pub(super) fn hierarchy(scope: &ConfigScope) -> Vec<ConfigScope> {
    let mut layers = vec![ConfigScope::Instance];
    match scope {
        ConfigScope::Instance => {}
        ConfigScope::Namespace { .. } => layers.push(scope.clone()),
        ConfigScope::Subject { namespace_id, .. } => {
            layers.push(ConfigScope::Namespace {
                namespace_id: namespace_id.clone(),
            });
            layers.push(scope.clone());
        }
    }
    layers
}

pub(super) fn affected(parent: &ConfigScope, candidate: &ConfigScope) -> bool {
    match parent {
        ConfigScope::Instance => true,
        ConfigScope::Namespace { namespace_id } => {
            matches!(candidate,ConfigScope::Namespace{namespace_id:id}|ConfigScope::Subject{namespace_id:id,..} if id==namespace_id)
        }
        ConfigScope::Subject { .. } => parent == candidate,
    }
}

pub(super) fn pending_keys(active: &ConfigSnapshot, desired: &ConfigSnapshot) -> Vec<String> {
    active
        .values()
        .keys()
        .filter(|key| {
            active.json(key) != desired.json(key) || active.source(key) != desired.source(key)
        })
        .cloned()
        .collect()
}
