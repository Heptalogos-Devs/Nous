//! Current quota authority is separate from a frozen invocation's model and Prompt identity.
use super::{CATALOG, ResourceMetadata, RoleRegistry};
use crate::configuration::{ConfigSnapshot, ConfigSource, Configuration};
use nous_domain::*;
use nous_storage::{Database, WriteTransaction};

#[derive(Clone)]
pub struct ModelBudgets {
    configuration: Configuration,
    roles: RoleRegistry,
}

impl ModelBudgets {
    pub fn new(configuration: Configuration, roles: RoleRegistry) -> Self {
        Self {
            configuration,
            roles,
        }
    }

    pub async fn plan_in(
        &self,
        tx: &mut WriteTransaction,
        job: &JobRecord,
        role: &str,
        resource: &ResourceMetadata,
    ) -> Result<ModelBudgetPlan> {
        let mut snapshots = Vec::new();
        for scope in hierarchy(job)? {
            snapshots.push(
                self.configuration
                    .snapshot_in_transaction(tx, &scope, false)
                    .await?,
            );
        }
        self.plan(job, role, resource, &snapshots)
    }

    pub async fn has_capacity(
        &self,
        database: &Database,
        job: &JobRecord,
        role: &str,
        resource: &ResourceMetadata,
        day: &str,
    ) -> Result<bool> {
        let mut view = database.read_view().await?;
        let mut snapshots = Vec::new();
        for scope in hierarchy(job)? {
            snapshots.push(
                self.configuration
                    .snapshot_in(&mut view, &scope, false)
                    .await?,
            );
        }
        let plan = self.plan(job, role, resource, &snapshots)?;
        let exhausted = view.exhausted_model_budgets(&plan, day).await?;
        view.finish().await?;
        Ok(exhausted.is_empty())
    }

    fn plan(
        &self,
        job: &JobRecord,
        role: &str,
        resource: &ResourceMetadata,
        snapshots: &[ConfigSnapshot],
    ) -> Result<ModelBudgetPlan> {
        let (namespace, subject) = job.subject_scope()?;
        let definition = self.roles.get(role)?;
        let instance = &snapshots[0];
        let catalog = instance.get(CATALOG)?;
        let provider = catalog
            .providers
            .get(&resource.provider)
            .ok_or_else(|| unavailable("the invocation's provider is no longer configured"))?;
        let model = catalog
            .models
            .get(&resource.id)
            .ok_or_else(|| unavailable("the invocation's model is no longer configured"))?;
        if !provider.enabled {
            return Err(unavailable("the invocation's provider is disabled"));
        }
        let mut limits = vec![
            ModelBudgetLimit {
                bucket: ModelBudgetBucket::Provider {
                    id: resource.provider.clone(),
                },
                max_calls_per_day: provider.max_calls_per_day,
            },
            ModelBudgetLimit {
                bucket: ModelBudgetBucket::Model {
                    id: resource.id.clone(),
                },
                max_calls_per_day: model.max_calls_per_day,
            },
        ];
        for snapshot in snapshots {
            let scope = snapshot.scope.clone();
            let binding = snapshot.get(definition.binding_key)?;
            if matches!(scope, ConfigScope::Subject { .. }) && binding.is_none() {
                return Err(unavailable(
                    "the model role no longer has an active binding",
                ));
            }
            let own_policy = scope == ConfigScope::Instance
                || matches!(snapshot.source(definition.binding_key.path()),Some(ConfigSource::Override{scope:owner,..}) if owner==&scope);
            if own_policy && let Some(binding) = binding {
                limits.push(ModelBudgetLimit {
                    bucket: ModelBudgetBucket::Role {
                        role: role.into(),
                        scope,
                    },
                    max_calls_per_day: binding.max_calls_per_day,
                });
            }
        }
        Ok(ModelBudgetPlan {
            context: ModelBudgetContext {
                provider_id: resource.provider.clone(),
                model_id: resource.id.clone(),
                role: role.into(),
                scope: ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: subject.clone(),
                },
            },
            limits,
        })
    }
}

fn hierarchy(job: &JobRecord) -> Result<[ConfigScope; 3]> {
    let (namespace, subject) = job.subject_scope()?;
    Ok([
        ConfigScope::Instance,
        ConfigScope::Namespace {
            namespace_id: namespace.clone(),
        },
        ConfigScope::Subject {
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
        },
    ])
}

fn unavailable(message: &str) -> Error {
    Error::new(ErrorCode::CapabilityUnavailable, message)
}
