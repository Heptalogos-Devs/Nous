// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use futures::{Stream, StreamExt};
use nous_core::{Error, Result};
use opendal::{Operator, services::Fs};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

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
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
            if exclusive {
                file.lock()
            } else {
                file.lock_shared()
            }
            .map_err(|e| Error::Infrastructure(e.to_string()))?;
            Ok(file)
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))?
    }

    pub async fn hashes(&self) -> Result<impl futures::Stream<Item = Result<String>> + use<>> {
        use futures::StreamExt;
        let lister = self
            .operator
            .lister_with("")
            .recursive(true)
            .await
            .map_err(storage_error)?;
        Ok(lister.filter_map(|entry| async move {
            match entry {
                Ok(entry) => {
                    let hash = entry.path().rsplit('/').next().unwrap_or("");
                    if Self::storage_key(hash).is_ok() {
                        Some(Ok(hash.to_string()))
                    } else {
                        None
                    }
                }
                Err(error) => Some(Err(storage_error(error))),
            }
        }))
    }

    pub async fn open(root: &str) -> Result<Self> {
        tokio::fs::create_dir_all(root)
            .await
            .map_err(|e| Error::Infrastructure(e.to_string()))?;
        let root = tokio::fs::canonicalize(root)
            .await
            .map_err(|e| Error::Infrastructure(e.to_string()))?;
        let operator =
            Operator::new(Fs::default().root(&root.to_string_lossy())).map_err(storage_error)?;
        Ok(Self { operator, root })
    }

    /// Canonical physical layout for a validated content digest.
    pub fn storage_key(hash: &str) -> Result<String> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(Error::Invalid("invalid BLAKE3 digest".into()));
        }
        Ok(format!("{}/{}", &hash[..2], hash))
    }

    pub async fn put(&self, bytes: Vec<u8>) -> Result<String> {
        let hash = blake3::hash(&bytes).to_hex().to_string();
        self.operator
            .write(&Self::storage_key(&hash)?, bytes)
            .await
            .map_err(storage_error)?;
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
            return Err(Error::Invalid("upload byte limit must be positive".into()));
        }
        let staging = self.root.join(format!(".staging-{}", uuid::Uuid::now_v7()));
        let mut file = tokio::fs::File::create(&staging)
            .await
            .map_err(|e| Error::Infrastructure(e.to_string()))?;
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
            let chunk_len = u64::try_from(chunk.len())
                .map_err(|_| Error::Invalid("upload chunk is too large".into()))?;
            size = size
                .checked_add(chunk_len)
                .ok_or_else(|| Error::Invalid("upload is too large".into()))?;
            if size > max_bytes {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::Invalid(
                    "upload exceeds configured byte limit".into(),
                ));
            }
            hasher.update(&chunk);
            if let Err(error) = file.write_all(&chunk).await {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::Infrastructure(error.to_string()));
            }
        }
        if let Err(error) = file.flush().await {
            let _ = tokio::fs::remove_file(&staging).await;
            return Err(Error::Infrastructure(error.to_string()));
        }
        drop(file);

        let hash = hasher.finalize().to_hex().to_string();
        let key = Self::storage_key(&hash)?;
        let target = self.root.join(&key);
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
        }
        match tokio::fs::metadata(&target).await {
            Ok(_) => {
                tokio::fs::remove_file(&staging)
                    .await
                    .map_err(|e| Error::Infrastructure(e.to_string()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tokio::fs::rename(&staging, &target)
                    .await
                    .map_err(|e| Error::Infrastructure(e.to_string()))?;
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&staging).await;
                return Err(Error::Infrastructure(error.to_string()));
            }
        }
        Ok((hash, size))
    }

    pub async fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let bytes = self
            .operator
            .read(&Self::storage_key(hash)?)
            .await
            .map_err(storage_error)?
            .to_vec();
        if blake3::hash(&bytes).to_hex().as_str() != hash {
            return Err(Error::Infrastructure(
                "artifact content hash mismatch".into(),
            ));
        }
        Ok(bytes)
    }

    pub async fn read_range(&self, hash: &str, start: u64, end: u64) -> Result<Vec<u8>> {
        if end < start {
            return Err(Error::Invalid("invalid byte range".into()));
        }
        if start == end {
            return Ok(Vec::new());
        }
        self.operator
            .read_with(&Self::storage_key(hash)?)
            .range(start..end)
            .await
            .map(|value| value.to_vec())
            .map_err(storage_error)
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
                std::str::from_utf8(&bytes[..error.valid_up_to()])
                    .map_err(|error| Error::Invalid(error.to_string()))?
            }
            Err(_) => return Ok(None),
        };
        Ok(Some(text.to_owned()))
    }

    /// Raw byte ranges never impose text boundaries; full reads verify CAS integrity at EOF.
    pub async fn stream(
        &self,
        hash: &str,
        range: Option<std::ops::Range<u64>>,
    ) -> Result<impl Stream<Item = Result<Vec<u8>>> + Send + use<>> {
        let mut file = tokio::fs::File::open(self.root.join(Self::storage_key(hash)?))
            .await
            .map_err(|e| Error::Infrastructure(e.to_string()))?;
        let remaining = if let Some(range) = &range {
            if range.end < range.start {
                return Err(Error::Invalid("invalid byte range".into()));
            }
            file.seek(std::io::SeekFrom::Start(range.start))
                .await
                .map_err(|e| Error::Infrastructure(e.to_string()))?;
            Some(range.end - range.start)
        } else {
            None
        };
        let expected = range.is_none().then(|| hash.to_owned());
        Ok(futures::stream::try_unfold(
            (file, blake3::Hasher::new(), expected, remaining),
            |(mut file, mut hasher, expected, remaining)| async move {
                if remaining == Some(0) {
                    return Ok(None);
                }
                let mut chunk = vec![0; remaining.unwrap_or(262_144).min(262_144) as usize];
                let size = file
                    .read(&mut chunk)
                    .await
                    .map_err(|e| Error::Infrastructure(e.to_string()))?;
                if size == 0 {
                    if remaining.is_some() {
                        return Err(Error::Infrastructure(
                            "artifact is shorter than requested range".into(),
                        ));
                    }
                    if let Some(expected) = expected
                        && hasher.finalize().to_hex().as_str() != expected
                    {
                        return Err(Error::Infrastructure(
                            "artifact content hash mismatch".into(),
                        ));
                    }
                    return Ok(None);
                }
                chunk.truncate(size);
                if expected.is_some() {
                    hasher.update(&chunk);
                }
                Ok(Some((
                    chunk,
                    (
                        file,
                        hasher,
                        expected,
                        remaining.map(|left| left - size as u64),
                    ),
                )))
            },
        ))
    }

    /// The canonical owner must first establish that no retained reference exists.
    pub async fn delete_unreferenced(&self, hash: &str) -> Result<()> {
        self.operator
            .delete(&Self::storage_key(hash)?)
            .await
            .map_err(storage_error)
    }

    pub async fn check(&self) -> Result<()> {
        self.operator.check().await.map_err(storage_error)
    }
}

fn storage_error(error: opendal::Error) -> Error {
    Error::Infrastructure(error.to_string())
}
