#[cfg(windows)]
mod definition;
#[cfg(target_os = "macos")]
mod launchd;
mod policy;
#[cfg(target_os = "linux")]
mod systemd;
#[cfg(windows)]
mod windows;
use super::{Platform, ProcessResult, ProcessSpec};
use crate::{
    configuration::ConfigWritePolicy,
    instance::{InstanceGuard, InstancePaths},
};
use nous_domain::{CallContext, Error, ErrorCode, Result};
pub use policy::{SERVICE, ServicePolicy, register_configuration};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    User,
    System,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    pub program: PathBuf,
    pub scope: ServiceScope,
    pub autostart: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServicePlan {
    pub label: String,
    pub manager: String,
    pub instance_root: PathBuf,
    pub program: PathBuf,
    pub scope: ServiceScope,
    pub autostart: bool,
    pub definition_path: PathBuf,
    pub definition: String,
    pub requires_user_session: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Uninstall,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ServiceStatus {
    pub label: String,
    pub manager: String,
    pub installed: bool,
    pub running: bool,
    pub instance_running: bool,
    pub native_state: String,
    pub last_exit_code: Option<i64>,
    pub requires_user_session: bool,
}
#[derive(Clone)]
pub struct Services {
    pub(super) paths: InstancePaths,
    pub(super) policy: ServicePolicy,
    pub(super) platform: Platform,
}
#[derive(Clone, Copy)]
enum RequestedState {
    Running,
    Stopped,
    Uninstalled,
}
impl Services {
    pub fn new(paths: InstancePaths, policy: ServicePolicy, platform: Platform) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            paths,
            policy,
            platform,
        })
    }
    pub fn plan(&self, ctx: &CallContext, spec: &ServiceSpec) -> Result<ServicePlan> {
        ConfigWritePolicy::LocalBootstrapOnly.require(ctx)?;
        if !spec.program.is_absolute() {
            return Err(Error::invalid(
                "service program must be an absolute executable path",
            ));
        }
        let program = InstancePaths::new(&spec.program)
            .map_err(super::installation::path_error)?
            .root;
        let label = format!(
            "org.heptalogos.nous-{}",
            &format!(
                "{:x}",
                Sha256::digest(self.paths.root.as_os_str().as_encoded_bytes())
            )[..16]
        );
        let (manager, definition_path, definition) = self.definition(&label, spec, &program)?;
        Ok(ServicePlan {
            label,
            manager,
            program,
            definition_path,
            definition,
            instance_root: self.paths.root.clone(),
            scope: spec.scope,
            autostart: spec.autostart,
            requires_user_session: spec.scope == ServiceScope::User && !cfg!(target_os = "linux"),
        })
    }
    pub async fn install(
        &self,
        ctx: &CallContext,
        spec: &ServiceSpec,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let plan = self.plan(ctx, spec)?;
        require_program(&plan.program)?;
        self.paths
            .ensure_layout()
            .map_err(super::installation::path_error)?;
        crate::secret::Secrets::new(std::sync::Arc::new(self.paths.clone()))?
            .prepare_instance_access()
            .await?;
        self.install_native(ctx, &plan, cancel).await?;
        self.status(ctx, spec, cancel).await
    }
    pub async fn action(
        &self,
        ctx: &CallContext,
        spec: &ServiceSpec,
        action: ServiceAction,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let plan = self.plan(ctx, spec)?;
        if matches!(action, ServiceAction::Start | ServiceAction::Restart) {
            require_program(&plan.program)?;
            crate::secret::Secrets::new(std::sync::Arc::new(self.paths.clone()))?
                .prepare_instance_access()
                .await?;
        }
        let deadline = self.deadline();
        let work = async {
            self.action_native(ctx, &plan, action, cancel).await?;
            let state = match action {
                ServiceAction::Start | ServiceAction::Restart => RequestedState::Running,
                ServiceAction::Stop => RequestedState::Stopped,
                ServiceAction::Uninstall => RequestedState::Uninstalled,
            };
            self.wait_for(ctx, &plan, state, deadline, cancel).await
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| deadline_error())?
    }
    pub async fn wait_stopped(
        &self,
        ctx: &CallContext,
        spec: &ServiceSpec,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let plan = self.plan(ctx, spec)?;
        self.wait_for(ctx, &plan, RequestedState::Stopped, self.deadline(), cancel)
            .await
    }
    fn deadline(&self) -> tokio::time::Instant {
        tokio::time::Instant::from_std(
            crate::clock::Clock.monotonic()
                + Duration::from_millis(self.policy.operation_timeout_ms),
        )
    }
    async fn wait_for(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        state: RequestedState,
        deadline: tokio::time::Instant,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let work = async {
            loop {
                crate::execution::check_cancelled(cancel)?;
                let status = self.status_native(ctx, plan, cancel).await?;
                let settled = match state {
                    RequestedState::Running => status.running && status.instance_running,
                    RequestedState::Stopped => !status.running && !status.instance_running,
                    RequestedState::Uninstalled => !status.installed,
                };
                if settled {
                    return Ok(status);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return Err(Error::new(ErrorCode::Cancelled, "service lifecycle operation cancelled")),
                    _ = tokio::time::sleep(Duration::from_millis(self.policy.status_poll_ms)) => {}
                }
            }
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| deadline_error())?
    }
    pub async fn status(
        &self,
        ctx: &CallContext,
        spec: &ServiceSpec,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let plan = self.plan(ctx, spec)?;
        self.status_native(ctx, &plan, cancel).await
    }
    pub(super) async fn run(
        &self,
        ctx: &CallContext,
        resource: &str,
        args: Vec<OsString>,
        cancel: &CancellationToken,
    ) -> Result<ProcessResult> {
        let mut spec = ProcessSpec::new(resource);
        spec.args = args;
        spec.timeout = Some(Duration::from_millis(self.policy.operation_timeout_ms));
        spec.host_session = true;
        self.platform.run(ctx, spec, cancel.clone()).await
    }
    pub(super) fn checked(
        &self,
        output: ProcessResult,
        plan: &ServicePlan,
    ) -> Result<ProcessResult> {
        if output.exit.success && matches!(output.reason, super::StopReason::Exited) {
            return Ok(output);
        }
        Err(Error::new(ErrorCode::DependencyUnavailable, "native service operation failed; system scope requires an elevated local account, user scope requires its user manager")
            .with_context("manager", plan.manager.clone()).with_context("exit_code", serde_json::json!(output.exit.code))
            .with_context("stop_reason", serde_json::json!(output.reason))
            .with_context("stderr", String::from_utf8_lossy(&output.stderr).into_owned()))
    }
    pub(super) fn status_value(
        &self,
        plan: &ServicePlan,
        installed: bool,
        running: bool,
        state: String,
        exit: Option<i64>,
    ) -> Result<ServiceStatus> {
        Ok(ServiceStatus {
            label: plan.label.clone(),
            manager: plan.manager.clone(),
            installed,
            running,
            native_state: state,
            instance_running: InstanceGuard::is_running(&self.paths)
                .map_err(super::installation::path_error)?,
            last_exit_code: exit,
            requires_user_session: plan.requires_user_session,
        })
    }
}

fn deadline_error() -> Error {
    Error::new(
        ErrorCode::DependencyUnavailable,
        "native service did not reach the requested state before the operation deadline",
    )
}
fn require_program(path: &Path) -> Result<()> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(Error::invalid("service program is not a file"));
    }
    Ok(())
}
pub(super) fn utf8(path: &Path) -> Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| Error::invalid("native service paths must be UTF8"))?;
    if value.chars().any(char::is_control) {
        return Err(Error::invalid(
            "native service paths contain control characters",
        ));
    }
    Ok(value
        .strip_prefix("\\\\?\\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(value))
}
pub(super) fn publish_definition(path: &Path, definition: &str, staging: &Path) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid("service definition has no parent"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::create_dir_all(staging)?;
    let mut file = tempfile::NamedTempFile::new_in(staging)?;
    file.write_all(definition.as_bytes())?;
    file.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o644))?;
    }
    file.persist(path)
        .map_err(|error| Error::dependency(error.error))?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
impl Services {
    fn unavailable() -> Error {
        Error::new(
            ErrorCode::CapabilityUnavailable,
            "native services are unsupported on this OS",
        )
    }
    fn definition(&self, _: &str, _: &ServiceSpec, _: &Path) -> Result<(String, PathBuf, String)> {
        Err(Self::unavailable())
    }
    async fn install_native(
        &self,
        _: &CallContext,
        _: &ServicePlan,
        _: &CancellationToken,
    ) -> Result<()> {
        Err(Self::unavailable())
    }
    async fn action_native(
        &self,
        _: &CallContext,
        _: &ServicePlan,
        _: ServiceAction,
        _: &CancellationToken,
    ) -> Result<()> {
        Err(Self::unavailable())
    }
    async fn status_native(
        &self,
        _: &CallContext,
        _: &ServicePlan,
        _: &CancellationToken,
    ) -> Result<ServiceStatus> {
        Err(Self::unavailable())
    }
}
