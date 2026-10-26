use super::{BINARY, MARKER, MARKER_CONTENT, artifact};
use fs2::FileExt;
use nous_domain::{Error, ErrorCode, Result};
use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};
use tokio_util::sync::CancellationToken;

pub(crate) fn digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::invalid("release digest must be lowercase SHA256"));
    }
    Ok(())
}
pub(crate) fn require_directory(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(Error::invalid("installation path must be a real directory"));
    }
    Ok(())
}
pub(crate) fn require_owned(root: &Path) -> Result<()> {
    require_directory(root)?;
    let marker = root.join(MARKER);
    if !fs::symlink_metadata(&marker)?.is_file() || fs::read(marker)? != MARKER_CONTENT {
        return Err(Error::invalid(
            "directory is not a Nous software installation",
        ));
    }
    for path in [
        root.join("releases"),
        root.join("temp"),
        root.join("temp/staging"),
    ] {
        require_directory(&path)?;
    }
    Ok(())
}
pub(crate) fn initialize(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    require_directory(root)?;
    if root.join(MARKER).exists() {
        return require_owned(root);
    }
    if fs::read_dir(root)?.next().is_some() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "program root must be empty or an existing Nous installation",
        ));
    }
    fs::create_dir_all(root.join("releases"))?;
    fs::create_dir_all(root.join("temp/staging"))?;
    let mut marker = File::options()
        .write(true)
        .create_new(true)
        .open(root.join(MARKER))?;
    marker.write_all(MARKER_CONTENT)?;
    marker.sync_all()?;
    Ok(())
}
pub(crate) fn lock(root: &Path) -> Result<File> {
    require_owned(root)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("temp/install.lock"))?;
    file.try_lock_exclusive().map_err(|error| {
        Error::new(
            ErrorCode::Conflict,
            "software installation is being modified",
        )
        .with_cause(&error)
    })?;
    Ok(file)
}
pub(crate) fn require_inventory(root: &Path) -> Result<()> {
    require_owned(root)?;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        if ![MARKER, BINARY, "releases", "temp"]
            .iter()
            .any(|allowed| name == *allowed)
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "program root contains files outside the installation inventory",
            ));
        }
    }
    reject_links(root)
}
fn reject_links(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(Error::invalid(
                "installation inventory contains a symbolic link",
            ));
        }
        if metadata.is_dir() {
            reject_links(&entry.path())?;
        }
    }
    Ok(())
}
pub(crate) fn activate(
    root: &Path,
    release: &Path,
    max: u64,
    cancel: &CancellationToken,
) -> Result<()> {
    let _lock = lock(root)?;
    let stage = tempfile::Builder::new()
        .prefix("publish-")
        .tempdir_in(root.join("temp/staging"))?;
    let prepared = stage.path().join(BINARY);
    artifact::copy(release, &prepared, max, cancel)?;
    let target = root.join(BINARY);
    if target.exists() {
        let previous_digest = artifact::hash(&target, max, cancel)?.0;
        let previous = root.join("releases").join(&previous_digest);
        if previous.exists() {
            require_directory(&previous)?;
            if artifact::hash(&previous.join(BINARY), max, cancel)?.0 != previous_digest {
                return Err(Error::invalid("previous release inventory is inconsistent"));
            }
        } else {
            let preserved = stage.path().join("previous");
            fs::create_dir(&preserved)?;
            artifact::copy(&target, &preserved.join(BINARY), max, cancel)?;
            fs::rename(preserved, previous)?;
        }
    }
    crate::execution::check_cancelled(cancel)?;
    #[cfg(unix)]
    {
        fs::rename(&prepared, &target)?;
    }
    #[cfg(not(unix))]
    {
        // Windows permits renaming a running image. Retain it in managed temp until its
        // process releases the image; no detached deletion helper or global TEMP is used.
        let retired = root
            .join("temp/staging")
            .join(format!("retired-{}", ulid::Ulid::generate()));
        let previous = target.exists();
        if previous {
            fs::rename(&target, &retired)?;
        }
        if let Err(error) = fs::rename(&prepared, &target) {
            if previous && let Err(rollback) = fs::rename(&retired, &target) {
                return Err(
                    Error::dependency(error).with_context("rollback_error", rollback.to_string())
                );
            }
            return Err(error.into());
        }
        if previous {
            let _ = fs::remove_file(retired);
        }
    }
    Ok(())
}
pub(crate) fn relocate(from: &Path, to: &Path, max: u64, cancel: &CancellationToken) -> Result<()> {
    let _lock = lock(from)?;
    require_inventory(from)?;
    if to.exists() {
        return Err(Error::new(
            ErrorCode::Conflict,
            "relocation destination already exists",
        ));
    }
    let parent = to
        .parent()
        .ok_or_else(|| Error::invalid("program root has no parent"))?;
    fs::create_dir_all(parent)?;
    let staging_parent = parent.join("temp/staging");
    fs::create_dir_all(&staging_parent)?;
    let staged = tempfile::Builder::new()
        .prefix("relocate-")
        .tempdir_in(staging_parent)?;
    let root = staged.path().join("program");
    initialize(&root)?;
    for entry in fs::read_dir(from.join("releases"))? {
        let entry = entry?;
        let release = entry.file_name().to_string_lossy().into_owned();
        digest(&release)?;
        require_directory(&entry.path())?;
        let destination = root.join("releases").join(&release);
        fs::create_dir(&destination)?;
        artifact::copy(
            &entry.path().join(BINARY),
            &destination.join(BINARY),
            max,
            cancel,
        )?;
        if artifact::hash(&destination.join(BINARY), max, cancel)?.0 != release {
            return Err(Error::invalid("release changed during relocation"));
        }
    }
    if from.join(BINARY).exists() {
        artifact::copy(&from.join(BINARY), &root.join(BINARY), max, cancel)?;
    }
    crate::execution::check_cancelled(cancel)?;
    fs::rename(&root, to)?;
    // Destination is fully published before the old software inventory is removed.
    // If Windows still holds an executable, return the source path so cleanup can be retried.
    fs::remove_dir_all(from).map_err(|error| {
        Error::dependency(error)
            .with_context("published_program_root", to.display().to_string())
            .with_context("cleanup_program_root", from.display().to_string())
    })?;
    Ok(())
}
