//! The instance owns all persistent and temporary locations.
use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::Serialize;
use std::{
    fs,
    path::{Component, Path, PathBuf},
};
#[derive(Debug, Clone, Serialize)]
pub struct InstancePaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub database: PathBuf,
    pub objects: PathBuf,
    pub indexes: PathBuf,
    pub logs: PathBuf,
    pub backups: PathBuf,
    pub exports: PathBuf,
    pub run: PathBuf,
    pub temp: PathBuf,
    pub credentials: PathBuf,
    pub prompts: PathBuf,
}

impl InstancePaths {
    pub fn default_path() -> Result<PathBuf> {
        Ok(ProjectDirs::from("org", "Heptalogos", "NousWave")
            .context("cannot determine the platform instance directory; pass --instance")?
            .data_local_dir()
            .join("default"))
    }

    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let root = resolve_existing_ancestor(path.as_ref())?;
        Ok(Self {
            config: root.join("instance.toml"),
            database: root.join("data/authority.sqlite"),
            objects: root.join("objects"),
            indexes: root.join("indexes"),
            logs: root.join("logs"),
            backups: root.join("backups"),
            exports: root.join("exports"),
            run: root.join("run"),
            temp: root.join("temp"),
            credentials: root.join("credentials"),
            prompts: root.join("prompts"),
            root,
        })
    }

    /// Resolve all managed paths and reject junctions/symlinks escaping this instance.
    pub fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let canonical_root = fs::canonicalize(&self.root)?;
        for path in [
            self.database.parent().expect("database has a parent"),
            &self.objects,
            &self.indexes,
            &self.logs,
            &self.backups,
            &self.exports,
            &self.run,
            &self.temp,
            &self.credentials,
            &self.prompts,
        ] {
            self.check_with_root(path, &canonical_root)?;
            fs::create_dir_all(path)?;
            self.check_with_root(path, &canonical_root)?;
        }
        for name in ["downloads", "cache", "staging", "install"] {
            let path = self.temp.join(name);
            self.check_with_root(&path, &canonical_root)?;
            fs::create_dir_all(path)?;
        }
        for path in [
            &self.config,
            &self.database,
            &self.run.join("instance.lock"),
        ] {
            self.check_with_root(path, &canonical_root)?;
        }
        Ok(())
    }

    fn check_with_root(&self, path: &Path, root: &Path) -> Result<()> {
        let resolved = resolve_existing_ancestor(path)?;
        if !is_within(&resolved, root) {
            bail!("managed path escapes the instance: {}", path.display());
        }
        Ok(())
    }

    pub fn resolve(&self, relative: impl AsRef<Path>) -> Result<PathBuf> {
        let relative = relative.as_ref();
        if relative.is_absolute()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            bail!("instance paths must be relative and may not contain parent components");
        }
        let path = self.root.join(relative);
        self.check_with_root(&path, &resolve_existing_ancestor(&self.root)?)?;
        Ok(path)
    }

    pub fn resolve_in(&self, owner_root: &str, relative: impl AsRef<Path>) -> Result<PathBuf> {
        let path = self.resolve(relative)?;
        let owner = self.resolve(owner_root)?;
        self.check_with_root(&path, &resolve_existing_ancestor(&owner)?)?;
        Ok(path)
    }
}

pub(super) fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(
            ancestor
                .file_name()
                .context("path has no existing ancestor")?
                .to_os_string(),
        );
        ancestor = ancestor.parent().context("path has no existing ancestor")?;
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for part in suffix.into_iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

pub(crate) fn is_within(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        path.to_string_lossy().to_lowercase() == root.to_string_lossy().to_lowercase()
            || path.to_string_lossy().to_lowercase().starts_with(&format!(
                "{}\\",
                root.to_string_lossy().trim_end_matches('\\').to_lowercase()
            ))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}
