use super::{ExecutableSource, Installations};
use nous_domain::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Write},
    path::Path,
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

impl Installations {
    pub(super) async fn acquire(
        &self,
        source: ExecutableSource,
        target: &Path,
        cancel: &CancellationToken,
    ) -> Result<()> {
        match source {
            ExecutableSource::Local { path } => {
                let target = target.to_owned();
                let max = self.policy.max_executable_bytes;
                crate::execution::blocking(cancel, move |cancel| copy(&path, &target, max, &cancel))
                    .await
            }
            ExecutableSource::Download { url, sha256 } => {
                super::filesystem::digest(&sha256)?;
                let parsed = reqwest::Url::parse(&url).map_err(Error::dependency)?;
                if parsed.scheme() != "https"
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                {
                    return Err(Error::invalid(
                        "software downloads require HTTPS without URL credentials",
                    ));
                }
                let network = self.network.clone();
                let max = self.policy.max_executable_bytes;
                let timeout = self.policy.download_timeout_ms;
                let target = target.to_owned();
                crate::execution::owned_async(cancel, move |cancel| async move {
                    let work = async {
                        let mut response = network.send(network.request(reqwest::Method::GET, &url)).await
                            .map_err(|error| Error::dependency(error.without_url()))?
                            .error_for_status().map_err(|error| Error::dependency(error.without_url()))?;
                        let mut file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&target).await?;
                        let mut hasher = Sha256::new();
                        let mut bytes = 0u64;
                        loop {
                            let chunk = tokio::select! {
                                _ = cancel.cancelled() => return Err(Error::new(ErrorCode::Cancelled, "software download cancelled")),
                                chunk = response.chunk() => chunk.map_err(|error| Error::dependency(error.without_url()))?,
                            };
                            let Some(chunk) = chunk else { break };
                            bytes = bytes.checked_add(chunk.len() as u64).ok_or_else(|| Error::invalid("software size overflow"))?;
                            if bytes > max { return Err(too_large()); }
                            hasher.update(&chunk);
                            file.write_all(&chunk).await?;
                        }
                        file.sync_all().await?;
                        if format!("{:x}", hasher.finalize()) != sha256 {
                            return Err(Error::invalid("downloaded executable does not match the supplied SHA256"));
                        }
                        Ok(())
                    };
                    tokio::select! {
                        _ = cancel.cancelled() => Err(Error::new(ErrorCode::Cancelled, "software download cancelled")),
                        result = tokio::time::timeout(std::time::Duration::from_millis(timeout), work) => {
                            result.map_err(|_| Error::new(ErrorCode::DependencyUnavailable, "software download timed out"))?
                        }
                    }
                }).await
            }
        }
    }
}
pub(super) fn copy(
    source: &Path,
    target: &Path,
    max: u64,
    cancel: &CancellationToken,
) -> Result<()> {
    if !std::fs::symlink_metadata(source)?.is_file() {
        return Err(Error::invalid("software source must be a regular file"));
    }
    let mut input = File::open(source)?;
    let mut output = File::options().write(true).create_new(true).open(target)?;
    let mut used = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        crate::execution::check_cancelled(cancel)?;
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        used = used
            .checked_add(read as u64)
            .ok_or_else(|| Error::invalid("software size overflow"))?;
        if used > max {
            return Err(too_large());
        }
        output.write_all(&buffer[..read])?;
    }
    output.sync_all()?;
    executable_permissions(target)?;
    Ok(())
}
pub(super) fn hash(path: &Path, max: u64, cancel: &CancellationToken) -> Result<(String, u64)> {
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(Error::invalid("installed artifact is not a regular file"));
    }
    let mut file = File::open(path)?;
    let mut bytes = 0u64;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        crate::execution::check_cancelled(cancel)?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| Error::invalid("software size overflow"))?;
        if bytes > max {
            return Err(too_large());
        }
        hash.update(&buffer[..read]);
    }
    if bytes == 0 {
        return Err(Error::invalid("software executable is empty"));
    }
    Ok((format!("{:x}", hash.finalize()), bytes))
}
pub(super) fn executable_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn too_large() -> Error {
    Error::new(
        ErrorCode::ResourceExhausted,
        "software executable exceeds installation policy",
    )
}
