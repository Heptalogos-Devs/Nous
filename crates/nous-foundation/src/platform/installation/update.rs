use super::{
    self as installation, BINARY, ExecutableSource, InstallationView, Installations, StagedUpdate,
};
use crate::instance::InstanceGuard;
use nous_domain::{CallContext, Error, Result};
use std::path::Path;
use tokio_util::sync::CancellationToken;

impl Installations {
    pub async fn stage(
        &self,
        ctx: &CallContext,
        program_root: &Path,
        source: ExecutableSource,
        cancel: &CancellationToken,
    ) -> Result<StagedUpdate> {
        Self::authorize(ctx)?;
        let root = self.program_root(program_root)?;
        let owner = self.clone();
        crate::execution::owned_async(cancel, move |cancel| async move {
            installation::filesystem::require_owned(&root)?;
            let workspace = tempfile::Builder::new()
                .prefix("download-")
                .tempdir_in(root.join("temp/staging"))?;
            let executable = workspace.path().join(BINARY);
            owner.acquire(source, &executable, &cancel).await?;
            let max = owner.policy.max_executable_bytes;
            crate::execution::blocking(&cancel, move |cancel| {
                let staging = workspace;
                let _lock = installation::filesystem::lock(&root)?;
                let (digest, bytes) = installation::artifact::hash(&executable, max, &cancel)?;
                installation::artifact::executable_permissions(&executable)?;
                let release = root.join("releases").join(&digest);
                if release.exists() {
                    installation::filesystem::require_directory(&release)?;
                    if installation::artifact::hash(&release.join(BINARY), max, &cancel)?.0
                        != digest
                    {
                        return Err(Error::invalid(
                            "existing release content does not match its digest",
                        ));
                    }
                } else {
                    let prepared = staging.path().join("release");
                    std::fs::create_dir(&prepared)?;
                    std::fs::rename(&executable, prepared.join(BINARY))?;
                    std::fs::rename(prepared, &release)?;
                }
                Ok(StagedUpdate {
                    program_root: root,
                    digest,
                    bytes,
                })
            })
            .await
        })
        .await
    }

    pub async fn activate(
        &self,
        ctx: &CallContext,
        staged: &StagedUpdate,
        cancel: &CancellationToken,
    ) -> Result<InstallationView> {
        Self::authorize(ctx)?;
        let owner = self.clone();
        let staged = staged.clone();
        crate::execution::owned_async(cancel, move |cancel| async move {
            owner
                .paths
                .ensure_layout()
                .map_err(installation::path_error)?;
            let _guard = InstanceGuard::acquire(&owner.paths).map_err(installation::path_error)?;
            owner.activate_locked(&staged, &cancel).await?;
            owner.inspect(&staged.program_root, &cancel).await
        })
        .await
    }

    pub async fn rollback(
        &self,
        ctx: &CallContext,
        program_root: &Path,
        digest: &str,
        cancel: &CancellationToken,
    ) -> Result<InstallationView> {
        installation::filesystem::digest(digest)?;
        self.activate(
            ctx,
            &StagedUpdate {
                program_root: program_root.into(),
                digest: digest.into(),
                bytes: 0,
            },
            cancel,
        )
        .await
    }

    pub(super) async fn activate_locked(
        &self,
        staged: &StagedUpdate,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let root = self.program_root(&staged.program_root)?;
        installation::filesystem::digest(&staged.digest)?;
        let expected = staged.digest.clone();
        let max = self.policy.max_executable_bytes;
        crate::execution::blocking(cancel, move |cancel| {
            installation::filesystem::require_owned(&root)?;
            let release = root.join("releases").join(&expected);
            installation::filesystem::require_directory(&release)?;
            let executable = release.join(BINARY);
            if installation::artifact::hash(&executable, max, &cancel)?.0 != expected {
                return Err(Error::invalid(
                    "release executable does not match the requested content digest",
                ));
            }
            installation::filesystem::activate(&root, &executable, max, &cancel)
        })
        .await
    }
}
