//! Business task handlers and readiness predicates.
use nous_domain::{Error, JobRecord, Result, validate_name};
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};
use tokio_util::sync::CancellationToken;

type TaskFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;
type Handler = Arc<dyn Fn(JobRecord, CancellationToken) -> TaskFuture + Send + Sync>;
type ReadyFuture = Pin<Box<dyn Future<Output = bool> + Send>>;
type Readiness = Arc<dyn Fn(JobRecord) -> ReadyFuture + Send + Sync>;
#[derive(Default)]
pub struct TaskRegistry {
    pub(super) handlers: HashMap<String, (Handler, Readiness)>,
}
impl TaskRegistry {
    pub fn register<F, Fut, R, RFut>(&mut self, kind: &str, handler: F, ready: R) -> Result<()>
    where
        F: Fn(JobRecord, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
        R: Fn(JobRecord) -> RFut + Send + Sync + 'static,
        RFut: Future<Output = bool> + Send + 'static,
    {
        validate_name(kind, "task kind")?;
        if self.handlers.contains_key(kind) {
            return Err(Error::invalid("task handler is already registered"));
        }
        self.handlers.insert(
            kind.into(),
            (
                Arc::new(move |job, cancel| Box::pin(handler(job, cancel))),
                Arc::new(move |job| Box::pin(ready(job))),
            ),
        );
        Ok(())
    }
}
