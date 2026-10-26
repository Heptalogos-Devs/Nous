use super::{PlatformConfig, resources, runner};
use crate::{clock::Clock, instance::InstancePaths};
use nous_domain::{CallContext, Error, ErrorCode, OperationTime, Result};
use process_wrap::tokio::CommandWrap;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    process::Command,
    sync::{Notify, Semaphore},
};
use tokio_util::sync::CancellationToken;

pub struct ProcessSpec {
    pub executable: String,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
}
impl ProcessSpec {
    pub fn new(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            stdin: None,
            timeout: None,
        }
    }
}

#[derive(Debug)]
pub struct ProcessWorkspace {
    pub(super) directory: tempfile::TempDir,
    pub(super) owner: std::path::PathBuf,
}
impl ProcessWorkspace {
    pub fn path(&self) -> &Path {
        self.directory.path()
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Exited,
    Cancelled,
    TimedOut,
    OutputLimit,
}
#[derive(Debug, serde::Serialize)]
pub struct ProcessExit {
    pub success: bool,
    pub code: Option<i32>,
    pub description: String,
}
#[derive(Debug)]
pub struct ProcessResult {
    pub id: String,
    pub exit: ProcessExit,
    pub reason: StopReason,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub started_at: OperationTime,
    pub completed_at: OperationTime,
    pub workspace: ProcessWorkspace,
}

pub struct ManagedProcess {
    pub id: String,
    trace_id: nous_domain::TraceId,
    stop: CancellationToken,
    result: Option<tokio::task::JoinHandle<Result<ProcessResult>>>,
}
impl ManagedProcess {
    pub fn stop(&self) {
        self.stop.cancel();
    }
    pub async fn wait(mut self) -> Result<ProcessResult> {
        self.result
            .take()
            .expect("process wait consumes the handle")
            .await
            .map_err(|error| Error::dependency(error).with_trace(self.trace_id.clone()))?
    }
}
impl Drop for ManagedProcess {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub(super) struct State {
    pub paths: InstancePaths,
    pub config: PlatformConfig,
    permits: Arc<Semaphore>,
    pub shutdown: CancellationToken,
    active: Mutex<BTreeSet<String>>,
    changed: Notify,
}
pub(super) struct ActiveProcess {
    state: Arc<State>,
    id: String,
}
impl Drop for ActiveProcess {
    fn drop(&mut self) {
        self.state
            .active
            .lock()
            .expect("managed process registry")
            .remove(&self.id);
        self.state.changed.notify_waiters();
    }
}

#[derive(Clone)]
pub struct Platform {
    pub(super) state: Arc<State>,
}
impl Platform {
    pub fn new(paths: InstancePaths, config: PlatformConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            state: Arc::new(State {
                paths,
                permits: Arc::new(Semaphore::new(config.max_parallel)),
                config,
                shutdown: CancellationToken::new(),
                active: Mutex::default(),
                changed: Notify::new(),
            }),
        })
    }
    pub fn workspace(&self) -> Result<ProcessWorkspace> {
        let parent = self
            .state
            .paths
            .resolve("temp/processes")
            .map_err(path_error)?;
        std::fs::create_dir_all(&parent).map_err(Error::dependency)?;
        let parent = self
            .state
            .paths
            .resolve("temp/processes")
            .map_err(path_error)?;
        let directory = tempfile::Builder::new()
            .prefix("process-")
            .tempdir_in(parent)
            .map_err(Error::dependency)?;
        super::permissions::protect_directory(directory.path())?;
        Ok(ProcessWorkspace {
            directory,
            owner: self.state.paths.root.clone(),
        })
    }
    pub async fn start(
        &self,
        ctx: &CallContext,
        workspace: ProcessWorkspace,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> Result<ManagedProcess> {
        self.launch(ctx, workspace, spec, cancel)
            .await
            .map_err(|error| error.with_trace(ctx.trace_id.clone()))
    }
    async fn launch(
        &self,
        ctx: &CallContext,
        workspace: ProcessWorkspace,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> Result<ManagedProcess> {
        if workspace.owner != self.state.paths.root {
            return Err(Error::invalid(
                "process workspace belongs to another instance",
            ));
        }
        let config = &self.state.config;
        let argument_bytes = spec
            .args
            .iter()
            .try_fold(0usize, |total, arg| {
                total.checked_add(arg.as_encoded_bytes().len())
            })
            .ok_or_else(|| Error::invalid("process argument size overflow"))?;
        if argument_bytes > config.max_argument_bytes
            || spec
                .stdin
                .as_ref()
                .is_some_and(|bytes| bytes.len() > config.max_stdin_bytes)
        {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "process input exceeds instance limits",
            ));
        }
        let timeout = spec
            .timeout
            .unwrap_or(Duration::from_millis(config.max_runtime_ms))
            .min(Duration::from_millis(config.max_runtime_ms));
        if timeout.is_zero() {
            return Err(Error::invalid("process timeout must be positive"));
        }
        let executable = self.executable(&spec.executable)?;
        let permit = tokio::select! {
            _=cancel.cancelled()=>return Err(Error::new(ErrorCode::Conflict,"process was cancelled before launch")),
            _=self.state.shutdown.cancelled()=>return Err(Error::new(ErrorCode::Conflict,"process service is stopping")),
            permit=self.state.permits.clone().acquire_owned()=>permit.map_err(Error::dependency)?,
        };
        let mut command = Command::new(executable);
        command
            .args(&spec.args)
            .current_dir(workspace.path())
            .env_clear()
            .envs(resources::environment(workspace.path()))
            .stdin(if spec.stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut command = CommandWrap::from(command);
        runner::wrap(&mut command);
        let started_at = Clock::operation_snapshot();
        let deadline = tokio::time::Instant::from_std(Clock.monotonic() + timeout);
        let child = command.spawn().map_err(Error::dependency)?;
        let id = ulid::Ulid::generate().to_string();
        let stop = self.state.shutdown.child_token();
        let active = ActiveProcess {
            state: self.state.clone(),
            id: id.clone(),
        };
        self.state
            .active
            .lock()
            .expect("managed process registry")
            .insert(id.clone());
        let supervisor = runner::Supervisor {
            state: self.state.clone(),
            trace_id: ctx.trace_id.clone(),
            id: id.clone(),
            tree: runner::ProcessTree::new(child),
            workspace,
            spec,
            deadline,
            stop: stop.clone(),
            cancel,
            permit,
            active,
            started_at,
        };
        let result = tokio::spawn(supervisor.run());
        Ok(ManagedProcess {
            id,
            trace_id: ctx.trace_id.clone(),
            stop,
            result: Some(result),
        })
    }
    pub async fn run(
        &self,
        ctx: &CallContext,
        spec: ProcessSpec,
        cancel: CancellationToken,
    ) -> Result<ProcessResult> {
        let workspace = self
            .workspace()
            .map_err(|error| error.with_trace(ctx.trace_id.clone()))?;
        self.start(ctx, workspace, spec, cancel).await?.wait().await
    }
    pub async fn shutdown(&self) {
        self.state.shutdown.cancel();
        loop {
            let changed = self.state.changed.notified();
            if self
                .state
                .active
                .lock()
                .expect("managed process registry")
                .is_empty()
            {
                break;
            }
            changed.await;
        }
    }
    pub fn status(&self) -> Result<serde_json::Value> {
        let active = self
            .state
            .active
            .lock()
            .map_err(|_| Error::internal("managed process registry lock poisoned"))?
            .clone();
        let resources = self
            .state
            .config
            .executables
            .keys()
            .map(|key| {
                let value = match self.executable(key) {
                    Ok(path) => serde_json::json!({"state":"ready","path":path}),
                    Err(error) => serde_json::json!({"state":"unavailable","error":error}),
                };
                (key.clone(), value)
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        Ok(
            serde_json::json!({"policy":self.state.config,"resources":resources,"active_processes":active,"stopping":self.state.shutdown.is_cancelled()}),
        )
    }
}

pub(super) fn path_error(error: anyhow::Error) -> Error {
    Error::new(
        ErrorCode::DependencyUnavailable,
        "managed process path is unavailable",
    )
    .with_cause(error.as_ref())
}
