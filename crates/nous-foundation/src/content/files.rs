//! Streaming integrity operations for explicitly supplied files.
use super::ObjectReader;
use crate::execution;
use nous_domain::{Error, ErrorCode, Result};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
use tokio_util::sync::CancellationToken;

pub async fn digest_file(path: &Path, cancel: &CancellationToken) -> Result<(String, u64)> {
    let path = path.to_owned();
    execution::blocking(cancel, move |local| {
        if !std::fs::metadata(&path)?.is_file() {
            return Err(Error::invalid("content source must be a regular file"));
        }
        let mut input = std::fs::File::open(path)?;
        let metadata = input.metadata()?;
        if !metadata.is_file() {
            return Err(Error::invalid("content source must be a regular file"));
        }
        let expected_len = metadata.len();
        let mut digest = Sha256::new();
        let mut byte_len = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            execution::check_cancelled(&local)?;
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            byte_len = byte_len
                .checked_add(count as u64)
                .ok_or_else(|| Error::invalid("file length overflow"))?;
            if byte_len > expected_len {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "content source grew while hashing",
                ));
            }
            digest.update(&buffer[..count]);
        }
        if byte_len != expected_len {
            return Err(Error::new(
                ErrorCode::Conflict,
                "content source shrank while hashing",
            ));
        }
        Ok((format!("{:x}", digest.finalize()), byte_len))
    })
    .await
}

pub async fn verify_file(
    source: &Path,
    hash: &str,
    byte_len: u64,
    cancel: &CancellationToken,
) -> Result<()> {
    let source = source.to_owned();
    let hash = hash.to_owned();
    let length = i64::try_from(byte_len)
        .map_err(|_| Error::invalid("file length does not fit authority"))?;
    execution::owned_async(cancel, move |local| async move {
        let mut input = ObjectReader::open(&source, &hash, length, None, local, None).await?;
        tokio::io::copy(&mut input, &mut tokio::io::sink()).await?;
        Ok(())
    })
    .await
}
