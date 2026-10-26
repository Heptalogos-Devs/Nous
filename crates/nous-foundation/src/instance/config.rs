//! Bootstrap file decoding; effective policy and provenance belong to Configuration.
use super::InstancePaths;
use crate::network::NetworkConfig;
use crate::{identity::AuthConfig, secret::Secrets};
use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
};
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct InstanceConfig {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub network: NetworkConfig,
    pub observability: crate::observability::ObservabilityConfig,
    pub tasks: crate::tasks::TaskRuntimeConfig,
    pub platform: crate::platform::PlatformConfig,
    pub media: crate::media::MediaPolicy,
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
        self.platform.validate()?;
        self.media.validate()?;
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

/// Initialize the bootstrap file and credential through their respective owners.
/// The caller holds InstanceGuard; all paths remain instance managed.
pub fn initialize(paths: &InstancePaths) -> Result<bool> {
    paths.ensure_layout()?;
    if paths.config.exists() {
        let config = InstanceConfig::load(paths)?;
        let value = Secrets::read_bootstrap_blocking(paths, &config.auth.admin_token)?;
        anyhow::ensure!(
            value.expose().len() >= 32,
            "bootstrap credential must contain at least 32 bytes"
        );
        return Ok(false);
    }
    let config = InstanceConfig::default();
    Secrets::ensure_bootstrap_blocking(paths, &config.auth.admin_token)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&paths.config)?;
    file.write_all(toml::to_string_pretty(&config)?.as_bytes())?;
    file.sync_all()?;
    Ok(true)
}
