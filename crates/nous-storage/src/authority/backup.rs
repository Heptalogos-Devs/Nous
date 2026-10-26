//! SQLite supplies the consistent authority snapshot; object bytes are copied
//! by the application from the snapshot's retained content references.
use std::{collections::BTreeMap, fs::OpenOptions, path::Path};

use nous_domain::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Database, ReadView, WriteTransaction, storage_error};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectReference {
    pub content_hash: String,
    pub byte_len: i64,
}

impl Database {
    /// Create a new SQLite snapshot without taking a write transaction. The
    /// existing pool's busy timeout applies while SQLite acquires its snapshot.
    /// The caller allocates the absolute destination within backup staging.
    pub async fn backup_to(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(Error::invalid(
                "backup destination must be an absolute file path",
            ));
        }
        let filename = path
            .to_str()
            .ok_or_else(|| Error::invalid("backup destination must be representable as UTF-8"))?;
        // Reserve the new filename atomically. VACUUM INTO accepts an empty
        // file, so an existing destination is never opened or overwritten.
        let output = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::new(ErrorCode::Conflict, "backup destination already exists")
                } else {
                    error.into()
                }
            })?;
        let snapshot = sqlx::query("VACUUM INTO ?")
            .bind(filename)
            .execute(&self.pool)
            .await
            .map_err(storage_error);
        let result = snapshot.and_then(|_| output.sync_all().map_err(Error::from));
        drop(output);
        if result.is_err() {
            // Only the file reserved by this invocation is removed on failure.
            let _ = std::fs::remove_file(path);
        }
        result
    }

    /// Read this database's actual object set. Backup callers invoke this on
    /// the opened snapshot, so later source commits cannot alter its manifest.
    pub async fn object_references(&self) -> Result<Vec<ObjectReference>> {
        let mut view = self.read_view().await?;
        let references = view.object_references().await?;
        view.finish().await?;
        Ok(references)
    }
}

macro_rules! object_references {
    ($owner:ty) => {
        impl $owner {
            /// Enumerate every retained original and derived artifact in this
            /// snapshot, including history and inputs of unfinished media work.
            pub async fn object_references(&mut self) -> Result<Vec<ObjectReference>> {
                let originals = sqlx::query("SELECT DISTINCT content_hash,byte_len FROM materials")
                    .fetch_all(&mut *self.tx)
                    .await
                    .map_err(storage_error)?;
                let mut lengths = BTreeMap::new();
                for row in originals {
                    retain_reference(
                        &mut lengths,
                        row.try_get("content_hash").map_err(storage_error)?,
                        row.try_get("byte_len").map_err(storage_error)?,
                    )?;
                }
                for artifact in self.media_artifact_references().await? {
                    let length = i64::try_from(artifact.byte_len)
                        .map_err(|_| Error::internal("artifact length exceeds authority range"))?;
                    retain_reference(&mut lengths, artifact.content_hash, length)?;
                }
                Ok(lengths
                    .into_iter()
                    .map(|(content_hash, byte_len)| ObjectReference {
                        content_hash,
                        byte_len,
                    })
                    .collect())
            }
        }
    };
}
object_references!(ReadView);
object_references!(WriteTransaction);

fn retain_reference(
    lengths: &mut BTreeMap<String, i64>,
    hash: String,
    byte_len: i64,
) -> Result<()> {
    if byte_len < 0 {
        return Err(Error::internal("referenced object length is negative"));
    }
    if let Some(previous) = lengths.insert(hash, byte_len)
        && previous != byte_len
    {
        return Err(Error::internal(
            "referenced object lengths disagree for one digest",
        ));
    }
    Ok(())
}
