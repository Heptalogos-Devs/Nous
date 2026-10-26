use super::{
    ServiceAction, ServicePlan, ServiceScope, ServiceSpec, ServiceStatus, Services,
    publish_definition, utf8,
};
use nous_domain::{CallContext, Error, ErrorCode, Result};
use plist::{Dictionary, Value};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

impl Services {
    pub(super) fn definition(
        &self,
        label: &str,
        spec: &ServiceSpec,
        program: &Path,
    ) -> Result<(String, PathBuf, String)> {
        let directory = self.paths.run.join("services");
        let mut values = Dictionary::new();
        let permissions =
            crate::platform::permissions::PrivatePermissions::for_instance(&self.paths.root)?;
        if spec.scope == ServiceScope::System {
            values.insert("UserName".into(), Value::String(permissions.owner_name()?));
        } else {
            permissions.require_owner_process()?;
        }
        values.insert("Label".into(), Value::String(label.into()));
        values.insert(
            "ProgramArguments".into(),
            Value::Array(vec![
                Value::String(utf8(program)?.into()),
                Value::String("--instance".into()),
                Value::String(utf8(&self.paths.root)?.into()),
                Value::String("serve".into()),
            ]),
        );
        values.insert(
            "WorkingDirectory".into(),
            Value::String(utf8(&self.paths.root)?.into()),
        );
        values.insert("RunAtLoad".into(), Value::Boolean(spec.autostart));
        let mut keep_alive = Dictionary::new();
        keep_alive.insert("SuccessfulExit".into(), Value::Boolean(false));
        values.insert("KeepAlive".into(), Value::Dictionary(keep_alive));
        values.insert(
            "ThrottleInterval".into(),
            Value::Integer(self.policy.restart_delay_ms.div_ceil(1000).into()),
        );
        values.insert(
            "ExitTimeOut".into(),
            Value::Integer(self.policy.stop_timeout_ms.div_ceil(1000).into()),
        );
        let mut environment = Dictionary::new();
        for name in ["TEMP", "TMP", "TMPDIR"] {
            environment.insert(name.into(), Value::String(utf8(&self.paths.temp)?.into()));
        }
        values.insert(
            "EnvironmentVariables".into(),
            Value::Dictionary(environment),
        );
        let mut xml = Vec::new();
        Value::Dictionary(values)
            .to_writer_xml(&mut xml)
            .map_err(Error::dependency)?;
        Ok((
            "launchd".into(),
            directory.join(format!("{label}.plist")),
            String::from_utf8(xml).map_err(Error::dependency)?,
        ))
    }
    pub(super) async fn install_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.platform.executable(&self.policy.launchctl_resource)?;
        publish_definition(
            &plan.definition_path,
            &plan.definition,
            &self.paths.temp.join("staging"),
        )?;
        let registration = self.registration_path(plan)?;
        std::fs::create_dir_all(registration.parent().expect("native plist has a parent"))?;
        if registration.symlink_metadata().is_ok() {
            if std::fs::read_link(&registration)? != plan.definition_path {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "launchd registration belongs to another target",
                ));
            }
        } else {
            std::os::unix::fs::symlink(&plan.definition_path, registration)?;
        }
        let _ = (ctx, cancel);
        Ok(())
    }
    pub(super) async fn action_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        action: ServiceAction,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let domain = self.launchd_domain(ctx, plan, cancel).await?;
        let target = format!("{domain}/{}", plan.label);
        let status = self.status_native(ctx, plan, cancel).await?;
        let loaded = status.native_state != "unloaded" && status.installed;
        if loaded
            && matches!(
                action,
                ServiceAction::Stop | ServiceAction::Restart | ServiceAction::Uninstall
            )
        {
            let output = self
                .run(
                    ctx,
                    &self.policy.launchctl_resource,
                    vec!["bootout".into(), target.clone().into()],
                    cancel,
                )
                .await?;
            self.checked(output, plan)?;
        }
        if matches!(action, ServiceAction::Start | ServiceAction::Restart) {
            if !loaded || matches!(action, ServiceAction::Restart) {
                let output = self
                    .run(
                        ctx,
                        &self.policy.launchctl_resource,
                        vec![
                            "bootstrap".into(),
                            domain.into(),
                            plan.definition_path.as_os_str().into(),
                        ],
                        cancel,
                    )
                    .await?;
                self.checked(output, plan)?;
            }
            let output = self
                .run(
                    ctx,
                    &self.policy.launchctl_resource,
                    vec!["kickstart".into(), target.into()],
                    cancel,
                )
                .await?;
            self.checked(output, plan)?;
        }
        if matches!(action, ServiceAction::Uninstall) && plan.definition_path.exists() {
            let registration = self.registration_path(plan)?;
            if std::fs::symlink_metadata(&registration).is_ok() {
                if std::fs::read_link(&registration)? != plan.definition_path {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "launchd registration belongs to another target",
                    ));
                }
                std::fs::remove_file(registration)?;
            }
            std::fs::remove_file(&plan.definition_path)?;
        }
        Ok(())
    }
    pub(super) async fn status_native(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<ServiceStatus> {
        let domain = self.launchd_domain(ctx, plan, cancel).await?;
        let output = self
            .run(
                ctx,
                &self.policy.launchctl_resource,
                vec!["print".into(), format!("{domain}/{}", plan.label).into()],
                cancel,
            )
            .await?;
        if !output.exit.success {
            if String::from_utf8_lossy(&output.stderr).contains("Could not find service") {
                let installed = plan.definition_path.exists();
                return self.status_value(
                    plan,
                    installed,
                    false,
                    if installed {
                        "unloaded"
                    } else {
                        "not_installed"
                    }
                    .into(),
                    None,
                );
            }
            return Err(self
                .checked(output, plan)
                .expect_err("unsuccessful process output"));
        }
        let text = String::from_utf8(output.stdout).map_err(Error::dependency)?;
        let property = |name: &str| {
            text.lines()
                .filter_map(|line| line.trim().split_once(" = "))
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_owned())
        };
        let state = property("state").unwrap_or_else(|| "unknown".into());
        let exit = property("last exit code").and_then(|value| value.parse().ok());
        self.status_value(plan, true, state == "running", state, exit)
    }
    async fn launchd_domain(
        &self,
        ctx: &CallContext,
        plan: &ServicePlan,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if plan.scope == ServiceScope::System {
            return Ok("system".into());
        }
        // launchctl accepts the owning GUI domain, which must be resolved from the actual OS uid.
        let output = self
            .run(ctx, &self.policy.id_resource, vec!["-u".into()], cancel)
            .await?;
        let output = self.checked(output, plan)?;
        let uid = String::from_utf8(output.stdout).map_err(Error::dependency)?;
        let uid = uid.trim().parse::<u32>().map_err(|error| {
            Error::new(
                ErrorCode::DependencyUnavailable,
                "id did not return a user id",
            )
            .with_cause(&error)
        })?;
        Ok(format!("gui/{uid}"))
    }
    fn registration_path(&self, plan: &ServicePlan) -> Result<PathBuf> {
        let root = if plan.scope == ServiceScope::User {
            directories::BaseDirs::new()
                .ok_or_else(|| Error::invalid("cannot determine user home"))?
                .home_dir()
                .join("Library/LaunchAgents")
        } else {
            PathBuf::from("/Library/LaunchDaemons")
        };
        Ok(root.join(format!("{}.plist", plan.label)))
    }
}
