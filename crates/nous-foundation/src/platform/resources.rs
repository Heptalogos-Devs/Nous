use super::{ExecutableResource, Platform};
use nous_domain::{Error, ErrorCode, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

impl Platform {
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
        let path = std::fs::canonicalize(path).map_err(Error::dependency)?;
        if !path.is_file() {
            return Err(Error::invalid("executable resource is not a file"));
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
