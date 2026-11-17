//! Bounded computation and immutable snapshot lifetime.
use crate::{CancellationToken, failure};
use anyhow::{Result, ensure};
use serde::Deserialize;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EngineSettings {
    pub threads: usize,
}
impl Default for EngineSettings {
    fn default() -> Self {
        Self {
            threads: std::thread::available_parallelism().map_or(1, |n| n.get().min(4)),
        }
    }
}
pub struct Engine {
    pool: rayon::ThreadPool,
    closed: AtomicBool,
    active: RwLock<()>,
}
impl Engine {
    pub fn new(settings: EngineSettings) -> Result<Arc<Self>> {
        ensure!(settings.threads > 0, "Engine threads must be positive");
        Ok(Arc::new(Self {
            pool: rayon::ThreadPoolBuilder::new()
                .num_threads(settings.threads)
                .build()?,
            closed: AtomicBool::new(false),
            active: RwLock::new(()),
        }))
    }
    pub fn run<T: Send>(
        &self,
        token: &CancellationToken,
        operation: impl FnOnce() -> Result<T> + Send,
    ) -> Result<T> {
        self.pool.install(|| {
            let _active = self
                .active
                .read()
                .map_err(|_| failure("wave.closed", "Engine lifecycle lock poisoned"))?;
            if self.closed.load(Ordering::Acquire) {
                return Err(failure("wave.closed", "Native engine is closed"));
            }
            token.check()?;
            operation()
        })
    }
    pub fn close(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        let _active = self
            .active
            .write()
            .map_err(|_| failure("wave.closed", "Engine lifecycle lock poisoned"))?;
        Ok(())
    }
}
