// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use futures::{Stream, StreamExt};
use nous_core::{Error, Result};
use opendal::{Operator, services::Fs};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct ObjectStore {
    operator: Operator,
    root: std::path::PathBuf,
}

impl ObjectStore {
    /// Shared guards cover byte writes through canonical reference commit. Purge
    /// takes an exclusive guard so it cannot race a shared-hash registration.
    pub async fn reference_guard(&self, exclusive: bool) -> Result<std::fs::File> {
        let path = self.root.join(".reference-lock");
        tokio::task::spawn_blocking(move || {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not open the object reference lock",
                    )
                    .with_source(e)
                })?;
            if exclusive {
                file.lock()
            } else {
                file.lock_shared()
            }
            .map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not acquire the object reference lock",
                )
                .with_source(e)
            })?;
            Ok(file)
        })
        .await
        .map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "The object reference lock worker failed",
            )
            .with_source(e)
        })?
    }

    pub async fn hashes(&self) -> Result<impl futures::Stream<Item = Result<String>> + use<>> {
        use futures::StreamExt;
        let lister = self
            .operator
            .lister_with("")
            .recursive(true)
            .await
            .map_err(|error| storage_error(error, "Could not list stored objects"))?;
        Ok(lister.filter_map(|entry| async move {
            match entry {
                Ok(entry) => {
                    let hash = entry.path().rsplit('/').next().unwrap_or("");
                    if Self::key(hash).is_ok() {
                        Some(Ok(hash.to_string()))
                    } else {
                        None
                    }
                }
                Err(error) => Some(Err(storage_error(error, "Could not list stored objects"))),
            }
        }))
    }

    pub async fn open(root: &str) -> Result<Self> {
        tokio::fs::create_dir_all(root).await.map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not initialize the object storage root",
            )
            .with_source(e)
        })?;
        let root = tokio::fs::canonicalize(root).await.map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not resolve the object storage root",
            )
            .with_source(e)
        })?;
        let operator =
            Operator::new(Fs::default().root(&root.to_string_lossy())).map_err(|error| {
                storage_error(error, "Could not initialize the object storage backend")
            })?;
        Ok(Self { operator, root })
    }

    fn key(hash: &str) -> Result<String> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "invalid BLAKE3 digest",
            ));
        }
        Ok(format!("{}/{}", &hash[..2], hash))
    }

    pub async fn put(&self, bytes: Vec<u8>) -> Result<String> {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        self.operator
            .write(&Self::key(&hash)?, bytes)
            .await
            .map_err(|error| storage_error(error, "Could not write the stored object"))?;
        Ok(hash)
    }

    /// Stream an object into a staging file, hashing and enforcing the bound
    /// while bytes arrive. The final rename is the only point at which the
    /// canonical content-addressed path becomes visible.
    pub async fn put_chunks<S>(&self, chunks: S, max_bytes: u64) -> Result<(String, u64)>
    where
        S: Stream<Item = Result<Vec<u8>>>,
    {
        if max_bytes == 0 {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "upload byte limit must be positive",
            ));
        }
        let staging = self.root.join(format!(".staging-{}", uuid::Uuid::now_v7()));
        let mut file = tokio::fs::File::create(&staging).await.map_err(|e| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not create the staged object file",
            )
            .with_source(e)
        })?;
        let mut hasher = blake3::Hasher::new();
        let mut size = 0_u64;
        futures::pin_mut!(chunks);
        while let Some(chunk) = chunks.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    let _ = tokio::fs::remove_file(&staging).await;
                    return Err(error);
                }
            };
            let chunk_len = u64::try_from(chunk.len()).map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "upload chunk is too large",
                )
                .with_source(error)
            })?;
            size = size.checked_add(chunk_len).ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "upload is too large",
                )
            })?;
            if size > max_bytes {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "Invalid request",
                ));
            }
            hasher.update(&chunk);
            if let Err(error) = file.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not write the staged object",
                )
                .with_source(error));
            }
        }
        if let Err(error) = file.flush().await {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not flush the staged object",
            )
            .with_source(error));
        }
        drop(file);

        let hash = hasher.finalize().to_hex().to_string();
        let key = Self::key(&hash)?;
        let target = self.root.join(&key);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not create the stored object directory",
                )
                .with_source(e)
            })?;
        }
        match tokio::fs::metadata(&target).await {
            Ok(_) => {
                tokio::fs::remove_file(&staging).await.map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not remove the redundant staged object",
                    )
                    .with_source(e)
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::rename(&staging, &target).await.map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not publish the stored object",
                    )
                    .with_source(e)
                })?;
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not inspect the existing object path",
                )
                .with_source(error));
            }
        }
        Ok((hash, size))
    }

    pub async fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = self
            .operator
            .read(&Self::key(hash)?)
            .await
            .map_err(|error| storage_error(error, "Could not read the stored object"))?
            .to_vec();
        if blake3::hash(&bytes).to_hex().as_str() != hash {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Stored object failed integrity verification",
            ));
        }
        Ok(bytes)
    }

    pub async fn read_range(&self, hash: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        if end < start {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "invalid byte range",
            ));
        }
        if start == end {
            return Ok(Vec::new());
        }
        self.operator
            .read_with(&Self::key(hash)?)
            .range(start..end)
            .await
            .map(|value| value.to_vec())
            .map_err(|error| storage_error(error, "Could not read the stored object range"))
    }

    /// Keep complete UTF-8 characters when a bounded read ends inside a character.
    pub async fn read_text_prefix(
        &self,
        hash: &str,
        byte_length: u64,
        limit: u64,
    ) -> Result<Option<String>> {
        let bytes = self.read_range(hash, 0, byte_length.min(limit)).await?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(text) => text,
            Err(error) if byte_length > limit && error.error_len().is_none() => {
                std::str::from_utf8(&bytes[..error.valid_up_to()]).map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InvalidRequest,
                        "Invalid request",
                    )
                    .with_source(error)
                })?
            }
            Err(_) => return Ok(None),
        };
        Ok(Some(text.to_owned()))
    }

    /// Read CAS bytes in bounded chunks and verify integrity at end of stream.
    pub async fn stream(
        &self,
        hash: &str,
    ) -> Result<impl Stream<Item = Result<Vec<u8>>> + Send + use<>> {
        let file = tokio::fs::File::open(self.root.join(Self::key(hash)?))
            .await
            .map_err(|e| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not open the stored object stream",
                )
                .with_source(e)
            })?;
        let expected = hash.to_owned();
        Ok(futures::stream::try_unfold(
            (file, blake3::Hasher::new(), expected),
            |(mut file, mut hasher, expected)| async move {
                let mut chunk = vec![0; 1024 * 1024];
                let size = file.read(&mut chunk).await.map_err(|e| {
                    Error::new(
                        nous_core::ErrorOwner::Resource,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not read the stored object stream",
                    )
                    .with_source(e)
                })?;
                if size == 0 {
                    if hasher.finalize().to_hex().as_str() != expected {
                        return Err(Error::new(
                            nous_core::ErrorOwner::Resource,
                            nous_core::ErrorCode::InternalFailure,
                            "Streamed object failed integrity verification",
                        ));
                    }
                    return Ok(None);
                }
                chunk.truncate(size);
                hasher.update(&chunk);
                Ok(Some((chunk, (file, hasher, expected))))
            },
        ))
    }

    /// The canonical owner must first establish that no retained reference exists.
    pub async fn delete_unreferenced(&self, hash: &str) -> Result<()> {
        self.operator
            .delete(&Self::key(hash)?)
            .await
            .map_err(|error| storage_error(error, "Could not delete the unreferenced object"))
    }

    pub async fn check(&self) -> Result<()> {
        self.operator
            .check()
            .await
            .map_err(|error| storage_error(error, "Could not check object storage"))
    }
}

fn storage_error(error: opendal::Error, message: &'static str) -> Error {
    Error::new(
        nous_core::ErrorOwner::Resource,
        nous_core::ErrorCode::InternalFailure,
        message,
    )
    .with_source(error)
}
