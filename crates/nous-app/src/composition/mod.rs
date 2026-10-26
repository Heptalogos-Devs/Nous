//! Register actual domain consumers and own their runtime lifetime.
use crate::App;
use nous_domain::Result;
use nous_foundation::models::RoleRegistry;
use nous_foundation::models::RoleRegistryBuilder;
use nous_foundation::tasks::TaskRegistry;
use tokio_util::sync::CancellationToken;

pub fn model_roles() -> Result<RoleRegistry> {
    let mut registry = RoleRegistryBuilder::default();
    registry.register(crate::cognition::memory::model::definition())?;
    for definition in crate::cognition::structured::model::definitions() {
        registry.register(definition)?;
    }
    registry.register(crate::retrieval::embedding_model::definition())?;
    Ok(registry.finish())
}

pub fn configuration_registry(
    roles: &RoleRegistry,
) -> Result<nous_foundation::configuration::ConfigRegistry> {
    let mut registry = nous_foundation::configuration::ConfigRegistryBuilder::new();
    nous_foundation::configuration::register_core(&mut registry)?;
    nous_foundation::observability::register_configuration(&mut registry)?;
    nous_foundation::tasks::register_configuration(&mut registry)?;
    nous_foundation::platform::register_configuration(&mut registry)?;
    nous_foundation::media::register_configuration(&mut registry)?;
    nous_foundation::models::register_configuration(&mut registry, roles)?;
    Ok(registry.finish())
}

impl App {
    fn task_handlers(&self) -> Result<TaskRegistry> {
        let mut registry = TaskRegistry::default();
        let memory = self.clone();
        let model = self.model.clone();
        let calls = self.calls.clone();
        registry.register(
            "memory.form",
            move |job, cancel| {
                let owner = memory.clone();
                async move { owner.run_memory_formation(job, cancel).await }
            },
            move |job| {
                let model = model.clone();
                let calls = calls.clone();
                async move {
                    calls
                        .ready(
                            &job,
                            "formation",
                            &model,
                            crate::cognition::memory::model::ROLE,
                            None,
                        )
                        .await
                }
            },
        )?;
        let cognition = self.clone();
        let readiness = self.clone();
        registry.register(
            "cognition.form",
            move |job, cancel| {
                let owner = cognition.clone();
                async move { owner.run_structured_formation(job, cancel).await }
            },
            move |job| {
                let owner = readiness.clone();
                async move { owner.structured_formation_ready(&job).await }
            },
        )?;
        let embedding = self.clone();
        let model = self.model.clone();
        let calls = self.calls.clone();
        registry.register(
            "embedding.text",
            move |job, cancel| {
                let owner = embedding.clone();
                async move { owner.run_embedding_job(job, cancel).await }
            },
            move |job| {
                let model = model.clone();
                let calls = calls.clone();
                async move {
                    calls
                        .ready(
                            &job,
                            "embedding",
                            &model,
                            crate::retrieval::embedding_model::ROLE,
                            None,
                        )
                        .await
                }
            },
        )?;
        Ok(registry)
    }
    pub fn start_background(&self, cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
        let app = self.clone();
        tokio::spawn(async move {
            if let Err(error) = app.calls.recover().await {
                tracing::error!(%error,"cannot recover model calls");
                return;
            }
            let registry = match app.task_handlers() {
                Ok(registry) => registry,
                Err(error) => {
                    tracing::error!(%error,"cannot register task handlers");
                    return;
                }
            };
            if let Err(error) = app.tasks.start(registry, cancel).await {
                tracing::error!(%error,"task host failed");
            }
        })
    }
}
