use super::{
    ServiceAction, ServicePlan, ServiceScope, ServiceSpec, ServiceStatus, Services,
    publish_definition, utf8,
};
use nous_domain::{CallContext, Error, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

impl Services {
    pub(super) fn definition(
        &self,
        label: &str,
        spec: &ServiceSpec,
        program: &Path,
    ) -> Result<(String, PathBuf, String)> {
        let directory = self.paths.run.join("services");
        let executable = quote(utf8(program)?);
        let instance = quote(utf8(&self.paths.root)?);
        let temp = quote(utf8(&self.paths.temp)?);
        let permissions =
            crate::platform::permissions::PrivatePermissions::for_instance(&self.paths.root)?;
        let identity = if spec.scope == ServiceScope::System {
            let (uid, gid) = permissions.owner_ids();
            format!("User={uid}\nGroup={gid}\n")
        } else {
            permissions.require_owner_process()?;
            String::new()
        };
        let definition = format!(
            "[Unit]\nDescription=Nous Wave {label}\n\n[Service]\nType=simple\n{identity}ExecStart={executable} --instance {instance} serve\nWorkingDirectory={instance}\nEnvironment=TEMP={temp}\nEnvironment=TMP={temp}\nEnvironment=TMPDIR={temp}\nRestart=on-failure\nRestartSec={}ms\nTimeoutStopSec={}ms\nKillMode=control-group\n\n[Install]\nWantedBy={}\n",
            self.policy.restart_delay_ms,
            self.policy.stop_timeout_ms,
            if spec.scope == ServiceScope::User {
                "default.target"
            } else {
                "multi-user.target"
            }
        );
        Ok((
            "systemd".into(),
            directory.join(format!("{label}.service")),
            definition,
        ))
    }
    pub(super) async fn install_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.platform.executable(&self.policy.systemctl_resource)?;
        publish_definition(
            &plan.definition_path,
            &plan.definition,
            &self.paths.temp.join("staging"),
        )?;
        // systemctl owns the native registration link; the source definition remains managed.
        if !plan.autostart {
            let status = self.status_native(ctx, plan, cancel).await?;
            if status.installed {
                self.systemctl(ctx, plan, &["disable", &plan.label], cancel)
                    .await?;
            }
        }
        self.systemctl(ctx, plan, &["link", utf8(&plan.definition_path)?], cancel)
            .await?;
        self.systemctl(ctx, plan, &["daemon-reload"], cancel)
            .await?;
        if plan.autostart {
            self.systemctl(ctx, plan, &["enable", &plan.label], cancel)
                .await?;
        }
        Ok(())
    }
    pub(super) async fn action_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        action: ServiceAction,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let command = match action {
            ServiceAction::Start => "start",
            ServiceAction::Stop => "stop",
            ServiceAction::Restart => "restart",
            ServiceAction::Uninstall => "disable",
        };
        if matches!(action, ServiceAction::Uninstall) {
            self.systemctl(ctx, plan, &["disable", "--now", &plan.label], cancel)
                .await?;
            if plan.definition_path.exists() {
                std::fs::remove_file(&plan.definition_path)?;
            }
            self.systemctl(ctx, plan, &["daemon-reload"], cancel)
                .await?;
        } else {
            self.systemctl(ctx, plan, &[command, &plan.label], cancel)
                .await?;
        }
        Ok(())
    }
    pub(super) async fn status_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let mut args = self.systemctl_args(plan);
        args.extend([
            "show".into(),
            format!("{}.service", plan.label).into(),
            "--property=LoadState,ActiveState,SubState,ExecMainStatus".into(),
        ]);
        let output = self
            .run(ctx, &self.policy.systemctl_resource, args, cancel)
            .await?;
        let output = self.checked(output, plan)?;
        let text = String::from_utf8(output.stdout).map_err(Error::dependency)?;
        let properties = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect::<std::collections::BTreeMap<_, _>>();
        let installed = properties
            .get("LoadState")
            .is_some_and(|state| *state != "not-found");
        let active = properties.get("ActiveState").copied().unwrap_or("unknown");
        let sub = properties.get("SubState").copied().unwrap_or("unknown");
        let exit = properties
            .get("ExecMainStatus")
            .and_then(|value| value.parse().ok());
        self.status_value(
            plan,
            installed,
            active == "active",
            format!("{active}/{sub}"),
            exit,
        )
    }
    fn systemctl_args(&self, plan: &ServicePlan) -> Vec<OsString> {
        if plan.scope == ServiceScope::User {
            vec!["--user".into()]
        } else {
            Vec::new()
        }
    }
    async fn systemctl(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        args: &[&str],
        cancel: &CancellationToken,
    ) -> Result<()> {
        let mut input = self.systemctl_args(plan);
        input.extend(args.iter().map(|argument| OsString::from(*argument)));
        let output = self
            .run(ctx, &self.policy.systemctl_resource, input, cancel)
            .await?;
        self.checked(output, plan)?;
        Ok(())
    }
}
fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('$', "$$")
            .replace('%', "%%")
    )
}
