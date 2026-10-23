//! Instance layout, bootstrap configuration, credentials and exclusive runtime ownership.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::SocketAddr,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs2::FileExt;
use nous_domain::{AccessScope, CallContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CONFIG_VERSION: u32 = 1;

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

    /// Called while holding InstanceGuard. Returns false for an existing valid instance.
    pub fn initialize(&self) -> Result<bool> {
        self.ensure_layout()?;
        if self.config.exists() {
            let config = InstanceConfig::load(self)?;
            self.read_secret(&config.auth.admin_token)?;
            return Ok(false);
        }
        protect_credentials_directory(&self.credentials)?;
        let config = InstanceConfig::default();
        let token_path = self.resolve(&config.auth.admin_token.file)?;
        // create_new preserves a credential left by an interrupted initialization.
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&token_path)
        {
            Ok(mut file) => {
                protect_secret_file(&token_path)?;
                let token = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                file.write_all(token.as_bytes())?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.read_secret(&config.auth.admin_token)?;
            }
            Err(e) => return Err(e.into()),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.config)?;
        file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
        file.sync_all()?;
        Ok(true)
    }

    pub fn read_secret(&self, reference: &SecretRef) -> Result<String> {
        let path = self.resolve(&reference.file)?;
        if !is_within(
            &resolve_existing_ancestor(&path)?,
            &resolve_existing_ancestor(&self.credentials)?,
        ) {
            bail!("credential references must be inside the instance credentials directory");
        }
        let mut token = String::new();
        File::open(&path)
            .with_context(|| {
                format!(
                    "cannot open credential reference {}",
                    reference.file.display()
                )
            })?
            .take(4097)
            .read_to_string(&mut token)?;
        let token = token.trim();
        if !(32..=4096).contains(&token.len()) || token.bytes().any(|b| b.is_ascii_whitespace()) {
            bail!("credential file must contain one token with 32 to 4096 bytes");
        }
        Ok(token.to_owned())
    }

    /// Issue while holding InstanceGuard. Only hashes and identity scope enter the registry.
    pub fn issue_token(&self, name: &str, scope: AccessScope) -> Result<serde_json::Value> {
        nous_domain::validate_name(name, "token name")?;
        let id = uuid::Uuid::new_v4().to_string();
        let reference = SecretRef {
            file: PathBuf::from(format!("credentials/{id}.token")),
        };
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        protect_credentials_directory(&self.credentials)?;
        let path = self.resolve(&reference.file)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        protect_secret_file(&path)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        let registry_path = self.resolve("credentials/tokens.json")?;
        let mut entries: Vec<TokenRecord> = if registry_path.exists() {
            serde_json::from_slice(&fs::read(&registry_path)?)?
        } else {
            Vec::new()
        };
        let context = CallContext {
            caller_id: format!("token:{id}"),
            scope,
        };
        entries.push(TokenRecord {
            name: name.to_owned(),
            digest: token_digest(&token),
            context: context.clone(),
        });
        let mut pending = tempfile::NamedTempFile::new_in(&self.credentials)?;
        pending.write_all(&serde_json::to_vec_pretty(&entries)?)?;
        pending.as_file().sync_all()?;
        pending.persist(&registry_path).map_err(|e| e.error)?;
        Ok(
            serde_json::json!({ "id": id, "name": name, "context": context, "token_file": path, "secret_ref": reference }),
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenRecord {
    pub name: String,
    pub digest: String,
    pub context: CallContext,
}

/// Snapshot loaded by the sole instance host. Offline token edits require stopping that host.
#[derive(Clone)]
pub struct TokenStore {
    records: Vec<TokenRecord>,
}

impl TokenStore {
    pub fn load(paths: &InstancePaths, config: &InstanceConfig) -> Result<Self> {
        let mut records = vec![TokenRecord {
            name: "administrator".into(),
            digest: token_digest(&paths.read_secret(&config.auth.admin_token)?),
            context: CallContext::admin(),
        }];
        let path = paths.resolve("credentials/tokens.json")?;
        if path.exists() {
            records.extend(serde_json::from_slice::<Vec<TokenRecord>>(&fs::read(
                path,
            )?)?);
        }
        Ok(Self { records })
    }

    pub fn authenticate(&self, token: &str) -> Option<CallContext> {
        use subtle::ConstantTimeEq;
        let digest = token_digest(token);
        self.records
            .iter()
            .find(|record| bool::from(record.digest.as_bytes().ct_eq(digest.as_bytes())))
            .map(|record| record.context.clone())
    }
}

fn token_digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    pub file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceConfig {
    pub format_version: u32,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub network: NetworkConfig,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            format_version: CONFIG_VERSION,
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl InstanceConfig {
    pub fn load(paths: &InstancePaths) -> Result<Self> {
        let text = fs::read_to_string(&paths.config)
            .context("instance is not initialized; run nous init --instance <path>")?;
        let config: Self = toml::from_str(&text).context("invalid instance.toml")?;
        config.validate()?;
        paths.resolve(&config.auth.admin_token.file)?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format_version != CONFIG_VERSION {
            bail!(
                "unsupported configuration version {}; this program supports {}",
                self.format_version,
                CONFIG_VERSION
            );
        }
        if self.server.max_body_bytes == 0 || self.server.max_body_bytes > 64 * 1024 * 1024 {
            bail!("server.max_body_bytes must be between 1 and 67108864");
        }
        if self.network.connect_timeout_seconds == 0 || self.network.request_timeout_seconds == 0 {
            bail!("network timeouts must be positive");
        }
        Ok(())
    }

    pub fn effective(paths: &InstancePaths) -> Result<serde_json::Value> {
        let config = Self::load(paths)?;
        let explicit: toml::Value = toml::from_str(&fs::read_to_string(&paths.config)?)?;
        let values = serde_json::to_value(&config)?;
        let mut sources = serde_json::Map::new();
        collect_sources("", &values, &explicit, &mut sources);
        Ok(serde_json::json!({
            "format_version": CONFIG_VERSION,
            "values": values,
            "sources": sources,
            "restart_required": ["server", "auth", "network"],
            "file": paths.config,
        }))
    }
}

fn collect_sources(
    prefix: &str,
    value: &serde_json::Value,
    explicit: &toml::Value,
    output: &mut serde_json::Map<String, serde_json::Value>,
) {
    if let serde_json::Value::Object(fields) = value {
        for (name, child) in fields {
            let key = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            collect_sources(&key, child, explicit, output);
        }
    } else {
        let found = prefix
            .split('.')
            .try_fold(explicit, |current, key| current.get(key));
        output.insert(
            prefix.to_owned(),
            serde_json::json!(if found.is_some() {
                "instance"
            } else {
                "builtin"
            }),
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub max_body_bytes: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:7421".parse().expect("constant socket address"),
            max_body_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AuthConfig {
    pub admin_token: SecretRef,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            admin_token: SecretRef {
                file: PathBuf::from("credentials/admin.token"),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub proxy: Option<String>,
    pub connect_timeout_seconds: u64,
    pub request_timeout_seconds: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            connect_timeout_seconds: 10,
            request_timeout_seconds: 120,
        }
    }
}

/// The file is retained on release so a competing process can never lock a replacement inode.
pub struct InstanceGuard {
    file: File,
}

impl InstanceGuard {
    pub fn acquire(paths: &InstancePaths) -> Result<Self> {
        let path = paths.run.join("instance.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock_exclusive()
            .context("instance is already in use by another host")?;
        Ok(Self { file })
    }

    pub fn is_running(paths: &InstancePaths) -> Result<bool> {
        let path = paths.run.join("instance.lock");
        if !path.exists() {
            return Ok(false);
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                FileExt::unlock(&file)?;
                Ok(false)
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.raw_os_error() == fs2::lock_contended_error().raw_os_error() =>
            {
                Ok(true)
            }
            Err(e) => Err(e.into()),
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn resolve_existing_ancestor(path: &Path) -> Result<PathBuf> {
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

fn is_within(path: &Path, root: &Path) -> bool {
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

#[cfg(unix)]
fn protect_credentials_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(windows)]
fn protect_credentials_directory(path: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let user = std::env::var("USERNAME").context("USERNAME is required to protect credentials")?;
    let account = match std::env::var("USERDOMAIN") {
        Ok(domain) => format!("{domain}\\{user}"),
        Err(_) => user,
    };
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .args([
            "/inheritance:r",
            "/grant:r",
            &format!("{account}:(OI)(CI)F"),
        ])
        .creation_flags(0x08000000)
        .output()
        .context("cannot set the credential directory ACL")?;
    if !status.status.success() {
        bail!(
            "cannot restrict credential directory ACL: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn protect_credentials_directory(_: &Path) -> Result<()> {
    bail!("credential protection is unavailable on this platform")
}

#[cfg(unix)]
fn protect_secret_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn protect_secret_file(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_lock_is_exclusive_and_config_has_sources() -> Result<()> {
        let root = std::env::temp_dir().join(format!("nous-instance-{}", uuid::Uuid::new_v4()));
        let paths = InstancePaths::new(&root)?;
        paths.ensure_layout()?;
        let guard = InstanceGuard::acquire(&paths)?;
        assert!(paths.initialize()?);
        assert!(!paths.initialize()?);
        assert!(InstanceGuard::acquire(&paths).is_err());
        assert!(InstanceGuard::is_running(&paths)?);
        let config = InstanceConfig::load(&paths)?;
        assert!(config.server.listen.ip().is_loopback());
        assert_eq!(paths.read_secret(&config.auth.admin_token)?.len(), 64);
        assert_eq!(
            InstanceConfig::effective(&paths)?["sources"]["server.listen"],
            "instance"
        );
        assert!(paths.resolve("../outside").is_err());
        drop(guard);
        assert!(!InstanceGuard::is_running(&paths)?);
        fs::remove_dir_all(root)?;
        Ok(())
    }
}
