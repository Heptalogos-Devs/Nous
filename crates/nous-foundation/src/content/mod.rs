//! Content-addressed streams and publication protection owned by the instance.
mod files;
mod lease;
mod reader;
mod writer;
use crate::{execution, instance::InstancePaths};
pub use files::{digest_file, verify_file};
pub use lease::ReferenceLease;
use nous_domain::{Error, ErrorCode, Result};
pub use reader::ObjectReader;
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;
pub use writer::copy_verified;

const STREAM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct ObjectStore {
    paths: Arc<InstancePaths>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PublishedObject {
    pub digest: String,
    pub byte_len: i64,
    #[serde(skip)]
    lease: ReferenceLease,
}

impl PublishedObject {
    pub fn protect_in(&self, operation: &mut nous_storage::WriteTransaction) {
        operation.retain_until_end(self.lease.clone());
    }
}

impl ObjectStore {
    pub fn new(paths: Arc<InstancePaths>) -> Self {
        Self { paths }
    }

    pub async fn protect_references(&self, cancel: &CancellationToken) -> Result<ReferenceLease> {
        lease::acquire(self.paths.clone(), cancel).await
    }

    pub async fn publish<R>(
        &self,
        reader: R,
        max_bytes: u64,
        cancel: &CancellationToken,
    ) -> Result<PublishedObject>
    where
        R: AsyncRead + Unpin + Send + 'static,
    {
        if max_bytes == 0 || max_bytes > i64::MAX as u64 {
            return Err(Error::invalid(
                "publication limit must be positive and fit the authority length",
            ));
        }
        let store = self.clone();
        execution::owned_async(cancel, move |local| async move {
            writer::publish(store, reader, max_bytes, local).await
        })
        .await
    }

    pub async fn publish_bytes(
        &self,
        bytes: &[u8],
        max_bytes: u64,
        cancel: &CancellationToken,
    ) -> Result<PublishedObject> {
        if bytes.len() as u64 > max_bytes {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "content exceeds publication limit",
            ));
        }
        self.publish(std::io::Cursor::new(bytes.to_vec()), max_bytes, cancel)
            .await
    }

    pub async fn open(
        &self,
        hash: &str,
        byte_len: i64,
        cancel: &CancellationToken,
    ) -> Result<ObjectReader> {
        self.open_range(hash, byte_len, None, cancel).await
    }

    pub async fn read_range(
        &self,
        hash: &str,
        byte_len: i64,
        range: Range<u64>,
        cancel: &CancellationToken,
    ) -> Result<ObjectReader> {
        self.open_range(hash, byte_len, Some(range), cancel).await
    }

    async fn open_range(
        &self,
        hash: &str,
        byte_len: i64,
        range: Option<Range<u64>>,
        cancel: &CancellationToken,
    ) -> Result<ObjectReader> {
        let path = self.path(hash)?;
        let lease = self.protect_references(cancel).await?;
        ObjectReader::open(&path, hash, byte_len, range, cancel.clone(), Some(lease)).await
    }

    pub async fn read_bytes(
        &self,
        hash: &str,
        byte_len: i64,
        max_bytes: u64,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        if byte_len < 0 || byte_len as u64 > max_bytes {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "content exceeds buffered read limit",
            ));
        }
        let store = self.clone();
        let hash = hash.to_owned();
        execution::owned_async(cancel, move |local| async move {
            let mut input = store.open(&hash, byte_len, &local).await?;
            let mut bytes = Vec::new();
            input.read_to_end(&mut bytes).await?;
            Ok(bytes)
        })
        .await
    }

    pub async fn copy_to(
        &self,
        hash: &str,
        byte_len: i64,
        target: &Path,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let lease = self.protect_references(cancel).await?;
        let source = self.path(hash)?;
        writer::copy_inner(
            self.paths.clone(),
            source,
            target.to_owned(),
            hash.to_owned(),
            byte_len,
            Some(lease),
            cancel,
        )
        .await
    }

    fn path(&self, hash: &str) -> Result<PathBuf> {
        validate_digest(hash)?;
        self.paths
            .resolve(format!("objects/{}/{hash}", &hash[..2]))
            .map_err(path_error)
    }
}

fn validate_digest(hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::invalid("object digest must be lowercase SHA-256"));
    }
    Ok(())
}

fn path_error(error: anyhow::Error) -> Error {
    Error::new(ErrorCode::DependencyUnavailable, error.to_string()).with_cause(error.as_ref())
}

fn managed_target(paths: &InstancePaths, target: &Path) -> Result<PathBuf> {
    let relative = if target.is_absolute() {
        target
            .strip_prefix(&paths.root)
            .map_err(|_| Error::invalid("content target must belong to the instance"))?
    } else {
        target
    };
    paths.resolve(relative).map_err(path_error)
}
