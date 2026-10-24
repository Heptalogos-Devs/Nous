//! Bootstrap file decoding; effective policy and provenance belong to Configuration.
use super::{
    InstancePaths, SecretRef,
    credentials::{protect_credentials_directory, protect_secret_file},
};
use crate::network::NetworkConfig;
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::PathBuf,
};
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceConfig {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub network: NetworkConfig,
    pub observability: crate::observability::ObservabilityConfig,
    pub tasks: crate::tasks::TaskRuntimeConfig,
    pub models: crate::models::ModelsDeployment,
}

impl InstanceConfig {
    pub fn load(paths: &InstancePaths) -> Result<Self> {
        let text = fs::read_to_string(&paths.config)
            .context("instance is not initialized; run nous init --instance <path>")?;
        Self::parse(&text, paths)
    }
    fn parse(text: &str, paths: &InstancePaths) -> Result<Self> {
        let config: Self = toml::from_str(text).context("invalid instance.toml")?;
        config.validate()?;
        paths.resolve(&config.auth.admin_token.file)?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        self.server.validate()?;
        self.auth.validate()?;
        self.network.validate()?;
        self.observability.validate()?;
        self.tasks.validate()?;
        self.models.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub listen: SocketAddr,
    pub max_body_bytes: usize,
}
impl ServerConfig {
    pub fn validate(&self) -> Result<()> {
        if self.max_body_bytes == 0 || self.max_body_bytes > 64 * 1024 * 1024 {
            bail!("server.max_body_bytes must be between 1 and 67108864");
        }
        Ok(())
    }
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
impl AuthConfig {
    pub fn validate(&self) -> Result<()> {
        self.admin_token.validate()
    }
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

impl InstancePaths {
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
}
