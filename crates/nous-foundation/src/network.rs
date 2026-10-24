//! Installation-wide outbound HTTP policy. Clone the client to share connection pools.
use std::time::Duration;

use anyhow::{Context, Result};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::Instrument;

#[derive(Clone)]
pub struct NetworkClients {
    http: reqwest::Client,
}

impl NetworkClients {
    pub fn new(config: &NetworkConfig) -> Result<Self> {
        config.validate()?;
        let mut builder = reqwest::Client::builder()
            .no_proxy()
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

    pub fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http.request(method, url)
    }

    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> std::result::Result<reqwest::Response, reqwest::Error> {
        let request = request.build()?;
        let span = tracing::info_span!("network.request",method=%request.method(),host=request.url().host_str().unwrap_or(""));
        self.http.execute(request).instrument(span).await
    }
    pub async fn read_bytes(
        &self,
        mut response: reqwest::Response,
        max_bytes: usize,
    ) -> nous_domain::Result<Vec<u8>> {
        if max_bytes == 0 {
            return Err(nous_domain::Error::invalid(
                "response byte limit must be positive",
            ));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            nous_domain::Error::dependency(error.without_url())
                .with_context("phase", "response_body")
        })? {
            if body.len().saturating_add(chunk.len()) > max_bytes {
                return Err(nous_domain::Error::new(
                    nous_domain::ErrorCode::ResourceExhausted,
                    "HTTP response exceeds its byte limit",
                )
                .with_context("max_bytes", max_bytes as u64));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
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

impl NetworkConfig {
    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.connect_timeout_seconds > 0 && self.request_timeout_seconds > 0,
            "network timeouts must be positive"
        );
        if let Some(proxy) = &self.proxy {
            reqwest::Proxy::all(proxy).context("invalid network.proxy")?;
        }
        Ok(())
    }
}
