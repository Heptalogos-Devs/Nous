//! Installation-wide outbound HTTP policy. Clone the client to share connection pools.
use std::time::Duration;

use anyhow::{Context, Result};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct NetworkClients {
    http: reqwest::Client,
}

impl NetworkClients {
    pub fn new(config: &NetworkConfig) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(concat!("NousWave/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(config.connect_timeout_seconds))
            .timeout(Duration::from_secs(config.request_timeout_seconds));
        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy).context("invalid network.proxy")?);
        }
        Ok(Self {
            http: builder
                .build()
                .context("cannot build outbound HTTP client")?,
        })
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
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
