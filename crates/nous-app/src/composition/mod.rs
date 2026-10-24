//! Register actual domain consumers and own their runtime lifetime.
use crate::App;
use crate::common::models::{RoleRegistry, RoleRegistryBuilder};
use crate::common::tasks::TaskRegistry;
use nous_domain::Result;
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
