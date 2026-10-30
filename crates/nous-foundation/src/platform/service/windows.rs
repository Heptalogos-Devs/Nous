use super::{
    ServiceAction, ServicePlan, ServiceScope, ServiceSpec, ServiceStatus, Services,
    definition::{Document, windows_argument},
    publish_definition, utf8,
};
use crate::platform::ProcessSpec;
use nous_domain::{CallContext, Error, ErrorCode, Result};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

impl Services {
    pub(super) fn definition(
        &self,
        label: &str,
        spec: &ServiceSpec,
        program: &Path,
    ) -> Result<(String, PathBuf, String)> {
        let mut doc = Document::new();
        let arguments = format!(
            "--instance {} serve",
            windows_argument(utf8(&self.paths.root)?)
        );
        match spec.scope {
            ServiceScope::User => {
                if self.policy.restart_delay_ms < 60_000 {
                    return Err(Error::invalid(
                        "Task Scheduler restart interval must be at least 60000 milliseconds",
                    ));
                }
                let permissions = crate::platform::permissions::PrivatePermissions::for_instance(
                    &self.paths.root,
                )?;
                let sid = permissions.owner_sid();
                doc.start(
                    "Task",
                    &[
                        ("version", "1.4"),
                        (
                            "xmlns",
                            "http://schemas.microsoft.com/windows/2004/02/mit/task",
                        ),
                    ],
                )?;
                doc.start("RegistrationInfo", &[])?;
                doc.text("Description", label)?;
                doc.end()?;
                doc.start("Triggers", &[])?;
                doc.start("LogonTrigger", &[])?;
                doc.text("Enabled", if spec.autostart { "true" } else { "false" })?;
                doc.text("UserId", sid)?;
                doc.end()?;
                doc.end()?;
                doc.start("Principals", &[])?;
                doc.start("Principal", &[("id", "Owner")])?;
                doc.text("UserId", sid)?;
                doc.text("LogonType", "InteractiveToken")?;
                doc.text("RunLevel", "LeastPrivilege")?;
                doc.end()?;
                doc.end()?;
                doc.start("Settings", &[])?;
                for (name, value) in [
                    ("MultipleInstancesPolicy", "IgnoreNew"),
                    ("DisallowStartIfOnBatteries", "false"),
                    ("StopIfGoingOnBatteries", "false"),
                    ("AllowHardTerminate", "true"),
                    ("StartWhenAvailable", "true"),
                    ("RunOnlyIfNetworkAvailable", "false"),
                    ("Enabled", "true"),
                    ("ExecutionTimeLimit", "PT0S"),
                ] {
                    doc.text(name, value)?;
                }
                doc.start("RestartOnFailure", &[])?;
                doc.text(
                    "Interval",
                    &format!("PT{}S", self.policy.restart_delay_ms.div_ceil(1000)),
                )?;
                doc.text("Count", &self.policy.windows_task_restart_count.to_string())?;
                doc.end()?;
                doc.end()?;
                doc.start("Actions", &[("Context", "Owner")])?;
                doc.start("Exec", &[])?;
                let shell = self.platform.executable(&self.policy.powershell_resource)?;
                let entry = self.host_entry_path(label);
                let hidden_arguments = [
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-WindowStyle",
                    "Hidden",
                    "-File",
                    utf8(&entry)?,
                    "-Program",
                    utf8(program)?,
                    "-Instance",
                    utf8(&self.paths.root)?,
                    "-HostArguments",
                    &arguments,
                    "-ManagedTemp",
                    utf8(&self.paths.temp)?,
                ]
                .into_iter()
                .map(windows_argument)
                .collect::<Vec<_>>()
                .join(" ");
                doc.text("Command", utf8(&shell)?)?;
                doc.text("Arguments", &hidden_arguments)?;
                doc.text("WorkingDirectory", utf8(&self.paths.root)?)?;
                doc.end()?;
                doc.end()?;
                doc.end()?;
                Ok((
                    "windows_task_scheduler".into(),
                    self.paths.run.join("services").join(format!("{label}.xml")),
                    doc.finish()?,
                ))
            }
            ServiceScope::System => {
                doc.start("service", &[])?;
                doc.text("id", label)?;
                doc.text("name", label)?;
                doc.text("description", "Nous cognitive service")?;
                doc.text("executable", utf8(program)?)?;
                doc.text("arguments", &arguments)?;
                doc.text("workingdirectory", utf8(&self.paths.root)?)?;
                doc.text("logpath", utf8(&self.paths.logs)?)?;
                doc.text(
                    "startmode",
                    if spec.autostart {
                        "Automatic"
                    } else {
                        "Manual"
                    },
                )?;
                doc.text(
                    "stoptimeout",
                    &format!("{} sec", self.policy.stop_timeout_ms.div_ceil(1000)),
                )?;
                doc.start(
                    "onfailure",
                    &[
                        ("action", "restart"),
                        (
                            "delay",
                            &format!("{} sec", self.policy.restart_delay_ms.div_ceil(1000)),
                        ),
                    ],
                )?;
                doc.end()?;
                for name in ["TEMP", "TMP"] {
                    doc.start("env", &[("name", name), ("value", utf8(&self.paths.temp)?)])?;
                    doc.end()?;
                }
                doc.end()?;
                Ok((
                    "windows_winsw".into(),
                    self.paths.run.join("services").join(format!("{label}.xml")),
                    doc.finish()?,
                ))
            }
        }
    }
    pub(super) async fn install_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<()> {
        publish_definition(
            &plan.definition_path,
            &plan.definition,
            &self.paths.temp.join("staging"),
        )?;
        if plan.scope == ServiceScope::User {
            publish_definition(
                &self.host_entry_path(&plan.label),
                HOST_ENTRY,
                &self.paths.temp.join("staging"),
            )?;
            self.task(ctx, plan, "install", cancel).await?;
        } else {
            let output = self
                .run(
                    ctx,
                    &self.policy.winsw_resource,
                    vec!["install".into(), plan.definition_path.as_os_str().into()],
                    cancel,
                )
                .await?;
            self.checked(output, plan)?;
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
            ServiceAction::Uninstall => "uninstall",
        };
        if plan.scope == ServiceScope::User {
            self.task(ctx, plan, command, cancel).await?;
        } else {
            if !plan.definition_path.exists() {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "WinSW service definition is not installed",
                ));
            }
            let output = self
                .run(
                    ctx,
                    &self.policy.winsw_resource,
                    vec![command.into(), plan.definition_path.as_os_str().into()],
                    cancel,
                )
                .await?;
            self.checked(output, plan)?;
        }
        if matches!(action, ServiceAction::Uninstall) && plan.definition_path.exists() {
            std::fs::remove_file(&plan.definition_path)?;
        }
        if matches!(action, ServiceAction::Uninstall) && plan.scope == ServiceScope::User {
            let entry = self.host_entry_path(&plan.label);
            if entry.exists() {
                std::fs::remove_file(entry)?;
            }
        }
        Ok(())
    }
    pub(super) async fn status_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let value = self.task(ctx, plan, "status", cancel).await?;
        let value: NativeStatus = serde_json::from_value(value)?;
        self.status_value(
            plan,
            value.installed,
            value.running,
            value.state,
            value.last_exit_code,
        )
    }
    async fn task(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        action: &str,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value> {
        let workspace = self.platform.workspace()?;
        let script = workspace.path().join("native-service.ps1");
        std::fs::write(&script, TASK_ADAPTER)?;
        let mut spec = ProcessSpec::new(&self.policy.powershell_resource);
        spec.timeout = Some(Duration::from_millis(self.policy.operation_timeout_ms));
        spec.host_session = true;
        spec.args = vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.into_os_string(),
            action.into(),
            plan.label.clone().into(),
            plan.definition_path.as_os_str().into(),
            if plan.scope == ServiceScope::User {
                "user".into()
            } else {
                "system".into()
            },
        ];
        let output = self
            .platform
            .start(ctx, workspace, spec, cancel.clone())
            .await?
            .wait()
            .await?;
        let output = self.checked(output, plan)?;
        if action == "status" {
            serde_json::from_slice(&output.stdout).map_err(Error::from)
        } else {
            Ok(serde_json::Value::Null)
        }
    }
    fn host_entry_path(&self, label: &str) -> PathBuf {
        self.paths
            .run
            .join("services")
            .join(format!("{label}-host.ps1"))
    }
}

#[derive(serde::Deserialize)]
struct NativeStatus {
    installed: bool,
    running: bool,
    state: String,
    last_exit_code: Option<i64>,
}

const TASK_ADAPTER: &str = r#"param([string]$Action,[string]$Label,[string]$Definition,[string]$Scope)
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
if($Scope -eq 'system') {
  if($Action -ne 'status'){throw 'Invalid system query action'}
  $service=Get-Service | Where-Object {$_.Name -eq $Label} | Select-Object -First 1
  if($null -eq $service){@{installed=$false;running=$false;state='not_installed'}|ConvertTo-Json -Compress}
  else{@{installed=$true;running=($service.Status -eq 'Running');state=$service.Status.ToString()}|ConvertTo-Json -Compress}
  exit
}
Import-Module ScheduledTasks
$task=Get-ScheduledTask | Where-Object {$_.TaskName -eq $Label -and $_.TaskPath -eq '\NousWave\'} | Select-Object -First 1
switch($Action) {
 'install' {Register-ScheduledTask -TaskName $Label -TaskPath '\NousWave\' -Xml ([IO.File]::ReadAllText($Definition)) -Force|Out-Null}
 'start' {if($null -eq $task){throw 'Task is not installed'}; $task|Start-ScheduledTask}
 'stop' {if($null -eq $task){throw 'Task is not installed'}; if($task.State -in @('Running','Queued')){$task|Stop-ScheduledTask}}
 'restart' {if($null -eq $task){throw 'Task is not installed'}; if($task.State -in @('Running','Queued')){$task|Stop-ScheduledTask}; $task|Start-ScheduledTask}
 'uninstall' {if($null -ne $task){if($task.State -in @('Running','Queued')){$task|Stop-ScheduledTask}; $task|Unregister-ScheduledTask -Confirm:$false}}
 'status' {
   if($null -eq $task){@{installed=$false;running=$false;state='not_installed'}|ConvertTo-Json -Compress}
   else{$info=$task|Get-ScheduledTaskInfo; @{installed=$true;running=($task.State -eq 'Running');state=$task.State.ToString();last_exit_code=$info.LastTaskResult}|ConvertTo-Json -Compress}
 }
 default {throw 'Unsupported native service action'}
}
"#;

const HOST_ENTRY: &str = r#"param([string]$Program,[string]$Instance,[string]$HostArguments,[string]$ManagedTemp)
$ErrorActionPreference='Stop'
$env:TEMP=$ManagedTemp
$env:TMP=$ManagedTemp
$env:TMPDIR=$ManagedTemp
$child=Start-Process -FilePath $Program -ArgumentList $HostArguments -WorkingDirectory $Instance -WindowStyle Hidden -PassThru -Wait
if($null -eq $child.ExitCode){throw 'Host did not return an exit code'}
exit $child.ExitCode
"#;
