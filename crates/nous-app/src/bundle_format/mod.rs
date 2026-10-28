//! Validated ZIP inventories and the external A-Memorix bundle format.

mod archive;
mod json;
mod manifest;
mod reference;
mod types;
use anyhow::Result;
pub(crate) use archive::read_archive;
pub(crate) use json::{canonical_images, canonical_json};
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
