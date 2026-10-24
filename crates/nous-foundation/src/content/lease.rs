//! Shared filesystem protection spans object publication and its authority registration.
use crate::{execution, instance::InstancePaths};
use nous_domain::Result;
use std::{fs::OpenOptions, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ReferenceLease {
    _file: Arc<std::fs::File>,
}

pub(super) async fn acquire(
    paths: Arc<InstancePaths>,
    cancel: &CancellationToken,
) -> Result<ReferenceLease> {
    execution::blocking(cancel, move |local| {
        let path = paths
            .resolve("run/object-references.lock")
            .map_err(super::path_error)?;
        std::fs::create_dir_all(paths.resolve("run").map_err(super::path_error)?)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        loop {
            execution::check_cancelled(&local)?;
            match fs2::FileExt::try_lock_shared(&file) {
                Ok(()) => {
                    return Ok(ReferenceLease {
                        _file: Arc::new(file),
                    });
                }
                Err(error) if crate::instance::lock_contended(&error) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(error) => return Err(error.into()),
            }
        }
    })
    .await
}
