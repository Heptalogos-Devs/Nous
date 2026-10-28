//! The official apalis persistence driver with a commit notification for task continuations.
use super::{JobEnvelope, queue_persistence::QueuePersistence};
use crate::JOB_QUEUE;
use apalis_core::{
    backend::{
        Backend, BackendConfig, WireFormatBackend,
        finalize::Durable,
        persistence::{Persisted, TaskPersistLayer},
    },
    task::Task,
    worker::context::WorkerContext,
};
use apalis_sqlite::{Config, Error, JsonCodec};
use serde_json::Value;
use sqlx::SqlitePool;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::Notify;

#[derive(Debug, Clone)]
pub struct QueueBackend {
    persistence: Persisted<QueuePersistence>,
    codec: JsonCodec<Vec<u8>>,
}
impl QueueBackend {
    pub(crate) fn new(pool: SqlitePool, wake: Arc<Notify>) -> Self {
        Self {
            persistence: Persisted::new(QueuePersistence {
                pool,
                config: Config::default().queue(JOB_QUEUE),
                wake,
            }),
            codec: JsonCodec::default(),
        }
    }
}
impl Backend for QueueBackend {
    type Task = Task<Vec<u8>>;
    type Error = Error;
    fn poll_ready(
        &mut self,
        cx: &mut Context<'_>,
        worker: &WorkerContext,
    ) -> Poll<Result<(), Error>> {
        self.persistence
            .poll_ready(cx, worker, self.persistence.config.heartbeat_interval)
    }
    fn poll_next(
        &mut self,
        cx: &mut Context<'_>,
        worker: &WorkerContext,
    ) -> Poll<Option<Result<Self::Task, Error>>> {
        self.persistence.poll_next(cx, worker)
    }
    fn poll_close(
        &mut self,
        cx: &mut Context<'_>,
        worker: &WorkerContext,
    ) -> Poll<Result<(), Error>> {
        self.persistence.poll_close(cx, worker)
    }
}
impl BackendConfig for QueueBackend {
    type Args = JobEnvelope;
    type Id = ulid::Ulid;
    type Kind = Durable;
    type Config = Config;
    type Layer = TaskPersistLayer<JsonCodec<Value>, Value>;
    fn config(&self) -> &Config {
        &self.persistence.config
    }
    fn middleware(&mut self, _: &mut WorkerContext) -> Self::Layer {
        self.persistence
            .layer(JsonCodec::default(), self.config().batch_size)
            .persist_results(self.config().persist_results)
            .lock_tasks(self.config().lock_tasks)
    }
}
impl WireFormatBackend for QueueBackend {
    type Codec = JsonCodec<Vec<u8>>;
    type Compact = Vec<u8>;
    fn codec(&self) -> &Self::Codec {
        &self.codec
    }
}
