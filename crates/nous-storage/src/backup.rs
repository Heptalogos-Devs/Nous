//! SQLite supplies the consistent authority snapshot; object bytes are copied
//! by the application from the snapshot's immutable material references.
use std::{fs::OpenOptions, path::Path};

use nous_domain::{Error, ErrorCode, Result};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::{Database, storage_error};

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
        sqlx::query(
            "SELECT DISTINCT content_hash,byte_len FROM materials ORDER BY content_hash,byte_len",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?
        .into_iter()
        .map(|row| {
            Ok(ObjectReference {
                content_hash: row.try_get("content_hash").map_err(storage_error)?,
                byte_len: row.try_get("byte_len").map_err(storage_error)?,
            })
        })
        .collect()
    }
}
