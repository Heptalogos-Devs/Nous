//! Bootstrap configuration is one current validated snapshot with field provenance.
use super::{
    InstancePaths, SecretRef,
    credentials::{protect_credentials_directory, protect_secret_file},
};
use crate::common::network::NetworkConfig;
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
    pub models: crate::common::models::ModelConfig,
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
        if self.server.max_body_bytes == 0 || self.server.max_body_bytes > 64 * 1024 * 1024 {
            bail!("server.max_body_bytes must be between 1 and 67108864");
        }
        if self.network.connect_timeout_seconds == 0 || self.network.request_timeout_seconds == 0 {
            bail!("network timeouts must be positive");
        }
        self.models.validate()?;
        Ok(())
    }

    pub fn effective(paths: &InstancePaths) -> Result<serde_json::Value> {
        let text = fs::read_to_string(&paths.config)?;
        let config = Self::parse(&text, paths)?;
        let explicit: toml::Value = toml::from_str(&text)?;
        let values = serde_json::to_value(&config)?;
        let mut sources = serde_json::Map::new();
        collect_sources("", &values, &explicit, &mut sources);
        Ok(serde_json::json!({
            "values": values,
            "sources": sources,
            "restart_required": ["server", "auth", "network", "models"],
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
