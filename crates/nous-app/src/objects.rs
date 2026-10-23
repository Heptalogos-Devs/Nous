//! Immutable local objects published before authority transactions commit.

use crate::instance::InstancePaths;
use nous_domain::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct ObjectStore {
    paths: Arc<InstancePaths>,
}

impl ObjectStore {
    pub fn new(paths: Arc<InstancePaths>) -> Self {
        Self { paths }
    }

    fn path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(Error::internal("invalid object digest in authority data"));
        }
        self.paths
            .resolve(format!("objects/{}/{hash}", &hash[..2]))
            .map_err(|error| Error::new(ErrorCode::DependencyUnavailable, error.to_string()))
    }

    pub fn publish(&self, bytes: &[u8]) -> Result<String> {
        let hash = format!("{:x}", Sha256::digest(bytes));
        let target = self.path(&hash)?;
        let staging = self
            .paths
            .resolve("temp/staging")
            .map_err(|error| Error::internal(error.to_string()))?;
        std::fs::create_dir_all(&staging)?;
        std::fs::create_dir_all(target.parent().expect("object parent"))?;
        self.path(&hash)?;
        let mut staged = tempfile::NamedTempFile::new_in(&staging)?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        match staged.persist_noclobber(&target) {
            Ok(_) => sync_parent(&target)?,
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error.into()),
        }
        // Verify both existing deduplicated content and a newly published object.
        self.read(&hash, bytes.len() as i64)?;
        Ok(hash)
    }

    pub fn read(&self, hash: &str, expected_len: i64) -> Result<Vec<u8>> {
        let bytes = std::fs::read(self.path(hash)?)?;
        if bytes.len() as i64 != expected_len || format!("{:x}", Sha256::digest(&bytes)) != hash {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "object integrity check failed",
            ));
        }
        Ok(bytes)
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    std::fs::File::open(path.parent().expect("object parent"))?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<()> {
    Ok(())
}
