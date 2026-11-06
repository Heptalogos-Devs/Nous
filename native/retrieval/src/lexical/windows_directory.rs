//! Windows atomic metadata replacement using the standard library's shared-reader semantics.
use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use tantivy::directory::{
    Directory, DirectoryLock, FileHandle, Lock, MmapDirectory, WatchCallback, WatchHandle,
    WritePtr,
    error::{DeleteError, LockError, OpenReadError, OpenWriteError},
};

/// Preserve Tantivy mmap, segment and lock behavior while publishing replaceable metadata.
#[derive(Clone, Debug)]
pub(super) struct WindowsDirectory {
    inner: MmapDirectory,
    root: PathBuf,
}

impl WindowsDirectory {
    pub(super) fn open(path: &Path) -> anyhow::Result<Self> {
        let root = path.canonicalize()?;
        Ok(Self {
            inner: MmapDirectory::open(&root)?,
            root,
        })
    }
}

impl Directory for WindowsDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        self.inner.get_file_handle(path)
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        self.inner.delete(path)
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        self.inner.exists(path)
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        self.inner.open_write(path)
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        self.inner.atomic_read(path)
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        // tempfile.persist uses MoveFileExW alone, which rejects a live reader even
        // when it shares deletion. std::fs::rename supplies the runtime's POSIX
        // replacement semantics. A normal file avoids tempfile's temporary attribute.
        let mut pending =
            tempfile::Builder::new().make_in(&self.root, |path: &Path| File::create_new(path))?;
        pending.write_all(data)?;
        pending.flush()?;
        pending.as_file().sync_data()?;
        let pending = pending.into_temp_path();
        std::fs::rename(&pending, self.root.join(path))
    }

    fn sync_directory(&self) -> io::Result<()> {
        self.inner.sync_directory()
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        self.inner.acquire_lock(lock)
    }

    fn watch(&self, callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        self.inner.watch(callback)
    }
}
