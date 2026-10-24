//! Bounded streaming writes publish only complete synced files.
use super::{
    ObjectReader, ObjectStore, PublishedObject, ReferenceLease, managed_target, path_error,
    validate_digest,
};
use crate::{execution, instance::InstancePaths};
use nous_domain::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub(super) async fn publish<R>(
    store: ObjectStore,
    mut input: R,
    max_bytes: u64,
    cancel: CancellationToken,
) -> Result<PublishedObject>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let lease = store.protect_references(&cancel).await?;
    let (staging, mut output) =
        staging_file(store.paths.clone(), Some(lease.clone()), &cancel).await?;
    let mut hasher = Sha256::new();
    let mut length = 0_u64;
    let mut chunk = [0_u8; 64 * 1024];
    let copied: Result<()> = async {
        loop {
            let count = tokio::select! {
                biased;
                _=cancel.cancelled()=>{ execution::check_cancelled(&cancel)?; unreachable!() },
                result=input.read(&mut chunk)=>result?,
            };
            if count == 0 {
                break;
            }
            length = length
                .checked_add(count as u64)
                .ok_or_else(|| Error::invalid("content length overflow"))?;
            if length > max_bytes {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "content exceeds publication limit",
                ));
            }
            hasher.update(&chunk[..count]);
            output.write_all(&chunk[..count]).await?;
        }
        Ok(())
    }
    .await;
    settle(&mut output, copied).await?;
    output.sync_all().await?;
    drop(output);
    let digest = format!("{:x}", hasher.finalize());
    let target = store.path(&digest)?;
    let publication_lease = lease.clone();
    let expected_hash = digest.clone();
    let paths = store.paths.clone();
    execution::blocking(&cancel, move |local| {
        let _lease = publication_lease;
        execution::check_cancelled(&local)?;
        std::fs::create_dir_all(target.parent().expect("object parent"))?;
        match staging.persist_noclobber(&target) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_existing(&target, &expected_hash, length, &local)?;
            }
            Err(error) => return Err(error.error.into()),
        }
        sync_parent(&target)?;
        sync_directory(&paths.objects)?;
        Ok(())
    })
    .await?;
    Ok(PublishedObject {
        digest,
        byte_len: length as i64,
        lease,
    })
}

pub async fn copy_verified(
    paths: Arc<InstancePaths>,
    source: &Path,
    target: &Path,
    hash: &str,
    byte_len: u64,
    cancel: &CancellationToken,
) -> Result<()> {
    validate_digest(hash)?;
    let length = i64::try_from(byte_len)
        .map_err(|_| Error::invalid("file length does not fit authority"))?;
    let lease = super::lease::acquire(paths.clone(), cancel).await?;
    copy_inner(
        paths,
        source.to_owned(),
        target.to_owned(),
        hash.to_owned(),
        length,
        Some(lease),
        cancel,
    )
    .await
}

pub(super) async fn copy_inner(
    paths: Arc<InstancePaths>,
    source: PathBuf,
    target: PathBuf,
    hash: String,
    byte_len: i64,
    lease: Option<ReferenceLease>,
    cancel: &CancellationToken,
) -> Result<()> {
    let target = managed_target(&paths, &target)?;
    execution::owned_async(cancel, move |local| async move {
        let mut input =
            ObjectReader::open(&source, &hash, byte_len, None, local.clone(), lease.clone())
                .await?;
        let (staging, mut output) = staging_file(paths, lease.clone(), &local).await?;
        let copied = tokio::io::copy(&mut input, &mut output)
            .await
            .map(|_| ())
            .map_err(Error::from);
        settle(&mut output, copied).await?;
        output.sync_all().await?;
        drop(output);
        execution::blocking(&local, move |local| {
            let _lease = lease;
            execution::check_cancelled(&local)?;
            std::fs::create_dir_all(
                target
                    .parent()
                    .ok_or_else(|| Error::invalid("file target needs a parent"))?,
            )?;
            staging
                .persist_noclobber(&target)
                .map_err(|error| Error::from(error.error))?;
            sync_parent(&target)
        })
        .await
    })
    .await
}

async fn settle(output: &mut tokio::fs::File, result: Result<()>) -> Result<()> {
    // A Tokio file write may still be running after poll_write accepted its buffer.
    // Flush before releasing its staging file or reference lease on every outcome.
    let flushed = output.flush().await.map_err(Error::from);
    if let Err(error) = &flushed
        && result.is_err()
    {
        tracing::warn!(event="content.flush_cleanup_failed", error=?error);
    }
    result.and(flushed)
}

async fn staging_file(
    paths: Arc<InstancePaths>,
    lease: Option<ReferenceLease>,
    cancel: &CancellationToken,
) -> Result<(tempfile::NamedTempFile, tokio::fs::File)> {
    execution::blocking(cancel, move |_local| {
        let _lease = lease;
        let directory = paths.resolve("temp/staging").map_err(path_error)?;
        std::fs::create_dir_all(&directory)?;
        let staging = tempfile::NamedTempFile::new_in(directory)?;
        let output = tokio::fs::File::from_std(staging.reopen()?);
        Ok((staging, output))
    })
    .await
}

fn verify_existing(
    path: &Path,
    hash: &str,
    expected_len: u64,
    cancel: &CancellationToken,
) -> Result<()> {
    use std::io::Read;
    let mut input = std::fs::File::open(path)?;
    if input.metadata()?.len() != expected_len {
        return Err(Error::new(
            ErrorCode::DependencyUnavailable,
            "existing object length differs from publication",
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        execution::check_cancelled(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    if format!("{:x}", digest.finalize()) != hash {
        return Err(Error::new(
            ErrorCode::DependencyUnavailable,
            "existing object hash differs from publication",
        ));
    }
    Ok(())
}

fn sync_parent(path: &Path) -> Result<()> {
    sync_directory(
        path.parent()
            .ok_or_else(|| Error::invalid("file target needs a parent"))?,
    )
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
