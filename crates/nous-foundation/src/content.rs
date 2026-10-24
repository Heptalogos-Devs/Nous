//! Immutable local objects published before authority transactions commit.

use crate::instance::InstancePaths;
use nous_domain::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct ObjectStore {
    paths: Arc<InstancePaths>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PublishedObject {
    pub digest: String,
    pub byte_len: i64,
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

    pub async fn publish(&self, bytes: Vec<u8>) -> Result<PublishedObject> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || store.publish_bytes(&bytes))
            .await
            .map_err(|error| Error::internal(error.to_string()))?
    }

    pub async fn read(&self, hash: &str, expected_len: i64) -> Result<Vec<u8>> {
        let store = self.clone();
        let hash = hash.to_owned();
        tokio::task::spawn_blocking(move || store.read_bytes(&hash, expected_len))
            .await
            .map_err(|error| Error::internal(error.to_string()))?
    }

    pub async fn copy_to(&self, hash: &str, expected_len: i64, target: &Path) -> Result<()> {
        let store = self.clone();
        let hash = hash.to_owned();
        let target = target.to_owned();
        tokio::task::spawn_blocking(move || {
            let parent = target
                .parent()
                .ok_or_else(|| Error::invalid("copy target needs a parent directory"))?;
            std::fs::create_dir_all(parent)?;
            let staging = store.paths.temp.join("staging");
            std::fs::create_dir_all(&staging)?;
            let mut output = tempfile::NamedTempFile::new_in(staging)?;
            let mut source = std::fs::File::open(store.path(&hash)?)?;
            let mut digest = Sha256::new();
            let mut length = 0_i64;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = source.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                length += read as i64;
                if length > expected_len {
                    return Err(Error::new(
                        ErrorCode::DependencyUnavailable,
                        "object length exceeds its authority record",
                    ));
                }
                digest.update(&buffer[..read]);
                output.write_all(&buffer[..read])?;
            }
            if length != expected_len || format!("{:x}", digest.finalize()) != hash {
                return Err(Error::new(
                    ErrorCode::DependencyUnavailable,
                    "object integrity check failed",
                ));
            }
            output.as_file().sync_all()?;
            output
                .persist_noclobber(&target)
                .map_err(|error| Error::from(error.error))?;
            sync_parent(&target)
        })
        .await
        .map_err(|error| Error::internal(error.to_string()))?
    }

    fn publish_bytes(&self, bytes: &[u8]) -> Result<PublishedObject> {
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
        self.read_bytes(&hash, bytes.len() as i64)?;
        Ok(PublishedObject {
            digest: hash,
            byte_len: bytes.len() as i64,
        })
    }

    fn read_bytes(&self, hash: &str, expected_len: i64) -> Result<Vec<u8>> {
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
