//! Cross-process ownership is held by one persistent lock file.
use super::InstancePaths;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
/// The file is retained on release so a competing process can never lock a replacement inode.
pub struct InstanceGuard {
    file: File,
}

impl InstanceGuard {
    pub fn acquire(paths: &InstancePaths) -> Result<Self> {
        let path = paths.run.join("instance.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock_exclusive()
            .context("instance is already in use by another host")?;
        Ok(Self { file })
    }

    pub fn is_running(paths: &InstancePaths) -> Result<bool> {
        let path = paths.run.join("instance.lock");
        if !path.exists() {
            return Ok(false);
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                FileExt::unlock(&file)?;
                Ok(false)
            }
            Err(e) if lock_contended(&e) => Ok(true),
            Err(e) => Err(e.into()),
        }
    }
}

pub(crate) fn lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
