use std::{
    io::{self, Write},
    sync::{Arc, RwLock},
};
use tracing_appender::non_blocking::{ErrorCounter, NonBlocking, WorkerGuard};
use tracing_subscriber::fmt::MakeWriter;

pub(super) struct Target {
    pub stderr: bool,
    pub file: Option<NonBlocking>,
    pub guard: Option<WorkerGuard>,
    pub dropped: Option<ErrorCounter>,
}

impl Default for Target {
    fn default() -> Self {
        Self {
            stderr: true,
            file: None,
            guard: None,
            dropped: None,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct LogWriter(pub Arc<RwLock<Target>>);

impl<'a> MakeWriter<'a> for LogWriter {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut target = self
            .0
            .write()
            .map_err(|_| io::Error::other("log writer lock poisoned"))?;
        if target.stderr {
            io::stderr().write_all(bytes)?;
        }
        if let Some(file) = target.file.as_mut() {
            file.write_all(bytes)?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let mut target = self
            .0
            .write()
            .map_err(|_| io::Error::other("log writer lock poisoned"))?;
        if target.stderr {
            io::stderr().flush()?;
        }
        if let Some(file) = target.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}
