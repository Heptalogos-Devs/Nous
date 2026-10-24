//! Protected credentials and scoped token snapshots.
use super::{
    InstanceConfig, InstancePaths,
    paths::{is_within, resolve_existing_ancestor},
};
use anyhow::{Context, Result, bail};
use nous_domain::{AccessScope, CallContext};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
impl InstancePaths {
    pub fn read_secret(&self, reference: &SecretRef) -> Result<String> {
        reference.validate()?;
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
        let mut pending = tempfile::NamedTempFile::new_in(self.resolve("temp/staging")?)?;
        protect_secret_file(pending.path())?;
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
impl SecretRef {
    pub fn validate(&self) -> Result<()> {
        if self.file.is_absolute()
            || !self.file.starts_with("credentials")
            || self.file.components().count() < 2
            || self
                .file
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            bail!("credential references must name a relative file inside credentials");
        }
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn protect_credentials_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn protect_credentials_directory(path: &Path) -> Result<()> {
    restrict_acl(path, "(OI)(CI)F")
}
#[cfg(windows)]
fn restrict_acl(path: &Path, rights: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let user = std::env::var("USERNAME").context("USERNAME is required to protect credentials")?;
    let account = match std::env::var("USERDOMAIN") {
        Ok(domain) => format!("{domain}\\{user}"),
        Err(_) => user,
    };
    let output = std::process::Command::new("icacls.exe")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("{account}:{rights}")])
        .creation_flags(0x08000000)
        .output()
        .context("cannot set credential ACL")?;
    if !output.status.success() {
        bail!(
            "cannot restrict credential ACL: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn protect_credentials_directory(_: &Path) -> Result<()> {
    bail!("credential protection is unavailable on this platform")
}

#[cfg(unix)]
pub(super) fn protect_secret_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
pub(super) fn protect_secret_file(path: &Path) -> Result<()> {
    restrict_acl(path, "F")
}

#[cfg(not(any(unix, windows)))]
pub(super) fn protect_secret_file(_: &Path) -> Result<()> {
    Ok(())
}
