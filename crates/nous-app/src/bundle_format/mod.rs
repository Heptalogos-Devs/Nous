//! A_Memorix v1/v2 archive inspection, without installation or filesystem extraction.
//!
//! Adapted from MaiBot/src/A_memorix/core/runtime/services/bundle_admin_service.py
//! at c602a2780f4f4dda4a6be46d67abc1cfb1ad420c: _load_bundle, member hashing,
//! _canonical_json_bytes, _canonical_image_semantics and RESOURCE_PRIMARY_KEYS.
//! SPDX-License-Identifier: AGPL-3.0-only. See licenses/A_memorix-LICENSE.
mod archive;
mod json;
mod manifest;
mod reference;
mod types;
use anyhow::Result;
use std::{
    fs::File,
    io::{Cursor, Read, Seek},
    path::Path,
};
pub use types::*;
pub fn inspect_file(path: &Path) -> Result<BundleInspection> {
    inspect(File::open(path)?, &Limits::default())
}
pub fn inspect_bytes(bytes: &[u8]) -> Result<BundleInspection> {
    inspect(Cursor::new(bytes), &Limits::default())
}
pub fn inspect<R: Read + Seek>(reader: R, limits: &Limits) -> Result<BundleInspection> {
    manifest::inspect_archive(archive::read_archive(reader, limits)?, limits)
}
