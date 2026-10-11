// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::{collections::BTreeMap, path::Path};

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Infrastructure(e.to_string()))?;
    std::fs::write(path, bytes).map_err(io)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).map_err(io)?;
    serde_json::from_slice(&bytes).map_err(|e| Error::Infrastructure(e.to_string()))
}

pub fn checksums(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    collect(root, root, &mut files)?;
    Ok(files)
}

fn collect(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        if kind.is_symlink() {
            return Err(Error::Infrastructure(
                "serving artifact contains a symbolic link".into(),
            ));
        }
        if kind.is_dir() {
            collect(root, &entry.path(), files)?;
        } else {
            let name = entry
                .path()
                .strip_prefix(root)
                .map_err(|e| Error::Infrastructure(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(entry.path()).map_err(io)?;
            files.insert(name, blake3::hash(&bytes).to_hex().to_string());
        }
    }
    Ok(())
}

pub fn digest(value: &impl Serialize) -> Result<String> {
    Ok(
        blake3::hash(&serde_json::to_vec(value).map_err(|e| Error::Infrastructure(e.to_string()))?)
            .to_hex()
            .to_string(),
    )
}

pub fn io(error: std::io::Error) -> Error {
    Error::Infrastructure(error.to_string())
}
