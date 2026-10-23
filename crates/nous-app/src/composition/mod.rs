//! Register actual domain consumers and own their runtime lifetime.
use crate::App;
use crate::common::tasks::TaskRegistry;
use nous_domain::{ModelRole, Result};
use tokio_util::sync::CancellationToken;

impl App {
    fn task_handlers(&self) -> Result<TaskRegistry> {
        let mut registry = TaskRegistry::default();
        let memory = self.clone();
        let model = self.model.clone();
        registry.register(
            "memory.form",
            move |job, cancel| {
                let owner = memory.clone();
                async move { owner.run_memory_formation(job, cancel).await }
            },
            move |wait| wait.kind == "configuration" && model.ready(ModelRole::MemoryFormation),
        )?;
        let cognition = self.clone();
        let model = self.model.clone();
        registry.register(
            "cognition.form",
            move |job, cancel| {
                let owner = cognition.clone();
                async move { owner.run_structured_formation(job, cancel).await }
            },
            move |wait| wait.kind == "configuration" && model.ready(ModelRole::StructuredFormation),
        )?;
        let embedding = self.clone();
        let model = self.model.clone();
        registry.register(
            "embedding.text",
            move |job, cancel| {
                let owner = embedding.clone();
                async move { owner.run_embedding_job(job, cancel).await }
            },
            move |wait| wait.kind == "configuration" && model.ready(ModelRole::TextEmbedding),
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
