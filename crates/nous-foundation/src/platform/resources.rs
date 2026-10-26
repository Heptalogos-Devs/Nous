use super::{ExecutableResource, Platform};
use nous_domain::{Error, ErrorCode, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

impl Platform {
    pub fn resource_configuration(&self, key: &str) -> Option<ExecutableResource> {
        self.state.config.executables.get(key).cloned()
    }
    pub fn executable(&self, key: &str) -> Result<PathBuf> {
        let resource = self.state.config.executables.get(key).ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "executable resource is not registered",
            )
            .with_context("resource", key)
        })?;
        let path = match resource {
            ExecutableResource::Managed { file } => self
                .state
                .paths
                .resolve(Path::new("bin").join(file))
                .map_err(super::process::path_error)?,
            ExecutableResource::External { path } => path.clone(),
        };
        let path = std::fs::canonicalize(path).map_err(|error| executable_error(key, error))?;
        let metadata = std::fs::metadata(&path).map_err(|error| executable_error(key, error))?;
        if !metadata.is_file() {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "executable resource is not a file",
            )
            .with_context("resource", key));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "executable resource has no execution permission",
                )
                .with_context("resource", key));
            }
        }
        #[cfg(windows)]
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
        {
            return Err(Error::invalid(
                "managed processes require a native executable, not a command script",
            ));
        }
        Ok(path)
    }
}

fn executable_error(key: &str, error: std::io::Error) -> Error {
    let code = if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) {
        ErrorCode::CapabilityUnavailable
    } else {
        ErrorCode::DependencyUnavailable
    };
    Error::new(code, "registered executable is unavailable")
        .with_context("resource", key)
        .with_cause(&error)
}

pub(super) fn environment(work: &Path) -> Vec<(OsString, OsString)> {
    // Adapted from Nous-previous apps/nous-core/src/process.ts's explicit inherited-name list.
    let mut vars = std::env::vars_os()
        .filter(|(key, _)| {
            matches!(
                key.to_string_lossy().to_ascii_lowercase().as_str(),
                "path"
                    | "systemroot"
                    | "systemdrive"
                    | "windir"
                    | "pathext"
                    | "lang"
                    | "lc_all"
                    | "lc_ctype"
                    | "tz"
            )
        })
        .collect::<Vec<_>>();
    for key in [
        "HOME",
        "USERPROFILE",
        "TEMP",
        "TMP",
        "TMPDIR",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "APPDATA",
        "LOCALAPPDATA",
        "CARGO_HOME",
        "NPM_CONFIG_CACHE",
        "PIP_CACHE_DIR",
    ] {
        vars.push((key.into(), work.as_os_str().to_owned()));
    }
    vars
}
