pub(super) mod artifact;
mod config;
mod update;
pub(super) mod filesystem;
use crate::{
    configuration::ConfigWritePolicy,
    instance::{self, InstanceGuard, InstancePaths},
    network::NetworkClients,
};
pub use config::{INSTALLATION, InstallationPolicy, register_configuration};
use nous_domain::{CallContext, Error, ErrorCode, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub(super) const MARKER: &str = ".nous-installation";
pub(super) const MARKER_CONTENT: &[u8] = b"NousWave software installation\n";
pub(super) const BINARY: &str = if cfg!(windows) { "nous.exe" } else { "nous" };

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutableSource {
    Local { path: PathBuf },
    Download { url: String, sha256: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallRequest {
    pub program_root: PathBuf,
    pub source: ExecutableSource,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StagedUpdate {
    pub program_root: PathBuf,
    pub digest: String,
    pub bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InstallationView {
    pub program_root: PathBuf,
    pub instance_root: PathBuf,
    pub executable: PathBuf,
    pub active_digest: Option<String>,
    pub releases: Vec<StagedUpdate>,
}

#[derive(Clone)]
pub struct Installations {
    pub(super) paths: Arc<InstancePaths>,
    pub(super) policy: InstallationPolicy,
    pub(super) network: NetworkClients,
}
impl Installations {
    pub fn new(
        paths: InstancePaths,
        policy: InstallationPolicy,
        network: NetworkClients,
    ) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            paths: Arc::new(paths),
            policy,
            network,
        })
    }

    pub async fn inspect(
        &self,
        program_root: &Path,
        cancel: &CancellationToken,
    ) -> Result<InstallationView> {
        let root = self.program_root(program_root)?;
        let instance_root = self.paths.root.clone();
        let max_bytes = self.policy.max_executable_bytes;
        crate::execution::blocking(cancel, move |cancel| {
            filesystem::require_owned(&root)?;
            let executable = root.join(BINARY);
            let active_digest = if executable.exists() {
                Some(artifact::hash(&executable, max_bytes, &cancel)?.0)
            } else {
                None
            };
            let mut releases = Vec::new();
            for entry in std::fs::read_dir(root.join("releases"))? {
                crate::execution::check_cancelled(&cancel)?;
                let entry = entry?;
                let digest = entry.file_name().to_string_lossy().into_owned();
                filesystem::digest(&digest)?;
                filesystem::require_directory(&entry.path())?;
                let (actual, bytes) =
                    artifact::hash(&entry.path().join(BINARY), max_bytes, &cancel)?;
                if actual != digest {
                    return Err(Error::invalid(
                        "installed release content does not match its digest",
                    ));
                }
                releases.push(StagedUpdate {
                    program_root: root.clone(),
                    digest,
                    bytes,
                });
            }
            releases.sort_by(|a, b| a.digest.cmp(&b.digest));
            Ok(InstallationView {
                program_root: root,
                instance_root,
                executable,
                active_digest,
                releases,
            })
        })
        .await
    }

    pub async fn install(
        &self,
        ctx: &CallContext,
        request: InstallRequest,
        cancel: &CancellationToken,
    ) -> Result<InstallationView> {
        Self::authorize(ctx)?;
        let owner = self.clone();
        let ctx = ctx.clone();
        crate::execution::owned_async(cancel, move |cancel| async move {
            let root = owner.program_root(&request.program_root)?;
            owner.paths.ensure_layout().map_err(path_error)?;
            let guard = InstanceGuard::acquire(&owner.paths).map_err(path_error)?;
            let owned_root = root.clone();
            crate::execution::blocking(&cancel, move |_| filesystem::initialize(&owned_root))
                .await?;
            let staged = owner.stage(&ctx, &root, request.source, &cancel).await?;
            owner.activate_locked(&staged, &cancel).await?;
            instance::initialize(&owner.paths).map_err(path_error)?;
            drop(guard);
            owner.inspect(&root, &cancel).await
        })
        .await
    }

    pub async fn relocate(
        &self,
        ctx: &CallContext,
        from: &Path,
        to: &Path,
        cancel: &CancellationToken,
    ) -> Result<InstallationView> {
        Self::authorize(ctx)?;
        let from = self.program_root(from)?;
        let to = self.program_root(to)?;
        if from == to || from.starts_with(&to) || to.starts_with(&from) {
            return Err(Error::invalid(
                "installation relocation roots must be disjoint",
            ));
        }
        self.paths.ensure_layout().map_err(path_error)?;
        let guard = InstanceGuard::acquire(&self.paths).map_err(path_error)?;
        let source = from.clone();
        let destination = to.clone();
        let max = self.policy.max_executable_bytes;
        crate::execution::blocking(cancel, move |cancel| {
            let _guard = guard;
            filesystem::relocate(&source, &destination, max, &cancel)
        })
        .await?;
        self.inspect(&to, cancel).await
    }

    pub async fn uninstall(
        &self,
        ctx: &CallContext,
        program_root: &Path,
        cancel: &CancellationToken,
    ) -> Result<()> {
        Self::authorize(ctx)?;
        let root = self.program_root(program_root)?;
        self.paths.ensure_layout().map_err(path_error)?;
        let guard = InstanceGuard::acquire(&self.paths).map_err(path_error)?;
        crate::execution::blocking(cancel, move |_| {
            let _guard = guard;
            filesystem::require_owned(&root)?;
            filesystem::require_inventory(&root)?;
            if std::env::current_exe()?.starts_with(&root) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "run uninstall from an executable outside this installation",
                ));
            }
            std::fs::remove_dir_all(&root)?;
            Ok(())
        })
        .await
    }

    pub(super) fn authorize(ctx: &CallContext) -> Result<()> {
        ConfigWritePolicy::LocalBootstrapOnly.require(ctx)
    }
    pub(super) fn program_root(&self, path: &Path) -> Result<PathBuf> {
        if !path.is_absolute() {
            return Err(Error::invalid("program root must be an absolute path"));
        }
        let root = InstancePaths::new(path).map_err(path_error)?.root;
        if root.starts_with(&self.paths.root) || self.paths.root.starts_with(&root) {
            return Err(Error::invalid(
                "program and instance data roots must be disjoint",
            ));
        }
        Ok(root)
    }
}
pub(super) fn path_error(error: anyhow::Error) -> Error {
    Error::new(ErrorCode::Conflict, error.to_string()).with_cause(error.as_ref())
}
