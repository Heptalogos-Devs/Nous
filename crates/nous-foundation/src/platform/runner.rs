// process-wrap Tokio composition adapted from upstream 82fc2561be449b28dfccff47b81429eb32753f06.
use super::{
    ProcessExit, ProcessResult, ProcessSpec, ProcessWorkspace, StopReason, io,
    process::{ActiveProcess, State},
};
use crate::clock::Clock;
use nous_domain::{Error, OperationTime, Result, TraceId};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use std::{
    process::ExitStatus,
    sync::{Arc, atomic::AtomicUsize},
    time::Duration,
};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

pub(super) fn wrap(command: &mut CommandWrap) {
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    {
        command.wrap(process_wrap::tokio::CreationFlags(
            windows::Win32::System::Threading::CREATE_NO_WINDOW,
        ));
        command.wrap(process_wrap::tokio::JobObject);
    }
}

pub(super) struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    armed: bool,
}
impl ProcessTree {
    pub(super) fn new(child: Box<dyn ChildWrapper>) -> Self {
        Self { child, armed: true }
    }
}
impl Drop for ProcessTree {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.child.start_kill();
        }
    }
}

// Own all launch resources before spawning so dropping an unpolled supervisor also kills the tree.
pub(super) struct Supervisor {
    pub state: Arc<State>,
    pub trace_id: TraceId,
    pub id: String,
    pub tree: ProcessTree,
    pub workspace: ProcessWorkspace,
    pub spec: ProcessSpec,
    pub deadline: tokio::time::Instant,
    pub stop: CancellationToken,
    pub cancel: CancellationToken,
    pub permit: OwnedSemaphorePermit,
    pub active: ActiveProcess,
    pub started_at: OperationTime,
}
impl Supervisor {
    pub async fn run(self) -> Result<ProcessResult> {
        let trace_id = self.trace_id.clone();
        let span = tracing::info_span!("platform.process",%trace_id,process_id=%self.id,resource=%self.spec.executable,pid=?self.tree.child.id());
        crate::observability::in_trace(trace_id.clone(), self.execute())
            .instrument(span)
            .await
            .map_err(|error| error.with_trace(trace_id))
    }
    async fn execute(self) -> Result<ProcessResult> {
        let Self {
            state,
            trace_id: _,
            id,
            mut tree,
            workspace,
            spec,
            deadline,
            stop,
            cancel,
            permit: _permit,
            active: _active,
            started_at,
        } = self;
        let stdout = tree.child.stdout().take();
        let stderr = tree.child.stderr().take();
        let stdin = tree.child.stdin().take();
        let overflow = CancellationToken::new();
        let total = Arc::new(AtomicUsize::new(0));
        let config = &state.config;
        let outputs = async {
            let (stdout, stderr, input) = tokio::join!(
                io::capture(
                    stdout,
                    config.max_output_bytes,
                    total.clone(),
                    overflow.clone()
                ),
                io::capture(stderr, config.max_output_bytes, total, overflow.clone()),
                io::input(stdin, spec.stdin)
            );
            input?;
            Ok::<_, std::io::Error>((stdout?, stderr?))
        };
        let cleanup = Duration::from_millis(config.cleanup_timeout_ms);
        let supervise = async {
            let selected = tokio::select! {
                status=tree.child.wait()=>Ok(status),
                _=stop.cancelled()=>Err(StopReason::Cancelled),
                _=cancel.cancelled()=>Err(StopReason::Cancelled),
                _=overflow.cancelled()=>Err(StopReason::OutputLimit),
                _=tokio::time::sleep_until(deadline)=>Err(StopReason::TimedOut),
            };
            match selected {
                Ok(status) => {
                    let _ = tree.child.start_kill();
                    status.map(|status| (StopReason::Exited, status))
                }
                Err(reason) => {
                    let status = terminate(
                        tree.child.as_mut(),
                        Duration::from_millis(config.stop_grace_ms),
                        cleanup,
                    )
                    .await?;
                    Ok((reason, status))
                }
            }
        };
        tracing::info!(event = "process.started", "managed process launched");
        let (finish, output) = tokio::join!(
            supervise,
            tokio::time::timeout_at(
                deadline + Duration::from_millis(config.stop_grace_ms) + cleanup,
                outputs
            )
        );
        // A successful leader must not leave helper children running or holding output pipes open.
        let _ = tree.child.start_kill();
        let (mut reason, status) = finish.map_err(Error::dependency)?;
        let (stdout, stderr) = output
            .map_err(Error::dependency)?
            .map_err(Error::dependency)?;
        if overflow.is_cancelled() && matches!(reason, StopReason::Exited) {
            reason = StopReason::OutputLimit;
        }
        tree.armed = false;
        let completed_at = Clock::operation_snapshot();
        tracing::info!(event="process.completed",?reason,code=?status.code(),"managed process finished");
        Ok(ProcessResult {
            id,
            exit: ProcessExit {
                success: status.success(),
                code: status.code(),
                description: status.to_string(),
            },
            reason,
            stdout,
            stderr,
            started_at,
            completed_at,
            workspace,
        })
    }
}

async fn terminate(
    child: &mut dyn ChildWrapper,
    grace: Duration,
    cleanup: Duration,
) -> std::io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        // POSIX SIGTERM is handled by process-wrap's process-group signal implementation.
        let _ = child.signal(15);
        if let Ok(Ok(status)) = tokio::time::timeout(grace, child.wait()).await {
            let _ = child.start_kill();
            return Ok(status);
        }
    }
    #[cfg(windows)]
    let _ = grace;
    child.start_kill()?;
    tokio::time::timeout(cleanup, child.wait())
        .await
        .map_err(std::io::Error::other)?
}
