//! Credential bytes are published immutably inside the instance credential directory.
use super::{SecretValue, Secrets};
use crate::{instance::InstancePaths, platform::permissions};
use nous_domain::{Error, Result, SecretRef};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub(super) fn path(paths: &InstancePaths, reference: &SecretRef) -> Result<PathBuf> {
    reference.validate()?;
    paths
        .resolve_in("credentials", &reference.file)
        .map_err(path_error)
}

pub(super) fn read(paths: &InstancePaths, reference: &SecretRef) -> Result<SecretValue> {
    let target = path(paths, reference)?;
    permissions::protect_file(&target)?;
    let file = fs::File::open(target)?;
    if !file.metadata()?.is_file() {
        return Err(Error::invalid("secret reference must name a regular file"));
    }
    let mut value = String::new();
    file.take(4097).read_to_string(&mut value)?;
    SecretValue::parse(value)
}

pub(super) fn initialize_directory(paths: &InstancePaths) -> Result<()> {
    let directory = paths.resolve("credentials").map_err(path_error)?;
    fs::create_dir_all(&directory)?;
    permissions::protect_directory(&directory)
}

pub(super) fn write_new(
    paths: &InstancePaths,
    reference: &SecretRef,
    value: &SecretValue,
) -> Result<()> {
    initialize_directory(paths)?;
    let target = path(paths, reference)?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::invalid("credential file needs a parent"))?;
    fs::create_dir_all(parent)?;
    permissions::protect_directory(parent)?;
    let directory = paths.resolve("temp/staging").map_err(path_error)?;
    fs::create_dir_all(&directory)?;
    let mut staging = tempfile::NamedTempFile::new_in(directory)?;
    permissions::protect_file(staging.path())?;
    staging.write_all(value.expose().as_bytes())?;
    staging.as_file().sync_all()?;
    staging
        .persist_noclobber(&target)
        .map_err(|error| Error::from(error.error))?;
    sync_parent(&target)
}

pub(super) fn ensure_bootstrap(
    paths: &InstancePaths,
    reference: &SecretRef,
) -> Result<SecretValue> {
    initialize_directory(paths)?;
    let target = path(paths, reference)?;
    if target.exists() {
        return read(paths, reference);
    }
    let value = Secrets::random_value();
    match write_new(paths, reference, &value) {
        Ok(()) => Ok(value),
        Err(error) if target.exists() => read(paths, reference).map_err(|_| error),
        Err(error) => Err(error),
    }
}

pub(super) fn path_error(source: anyhow::Error) -> Error {
    Error::new(
        nous_domain::ErrorCode::DependencyUnavailable,
        source.to_string(),
    )
    .with_cause(source.as_ref())
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<()> {
    fs::File::open(
        path.parent()
            .ok_or_else(|| Error::invalid("credential path needs a parent"))?,
    )?
    .sync_all()?;
    Ok(())
}
#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> Result<()> {
    Ok(())
}
