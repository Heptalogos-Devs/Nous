use super::Operation;
use nous_domain::{Error, ErrorCode, TraceId};
use nous_foundation::network::NetworkClients;
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Clone)]
pub struct ApiClient {
    network: NetworkClients,
    base_url: String,
    token: Arc<String>,
}
impl ApiClient {
    pub fn new(network: NetworkClients, base_url: String, token: String) -> anyhow::Result<Self> {
        let url: reqwest::Url = base_url.parse()?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host().is_some(),
            "service URL must use HTTP or HTTPS"
        );
        anyhow::ensure!(
            url.username().is_empty() && url.password().is_none(),
            "service URL may not contain credentials"
        );
        Ok(Self {
            network,
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: Arc::new(token),
        })
    }
    pub async fn execute(&self, operation: &Operation) -> anyhow::Result<Value> {
        Ok(self.execute_domain(operation).await?)
    }
    pub async fn execute_domain(&self, operation: &Operation) -> Result<Value, Error> {
        let (method, path, body) = operation.request();
        self.request(method, &path, body).await
    }
    pub async fn stop(&self) -> anyhow::Result<Value> {
        Ok(self.request("POST", "/api/stop", None).await?)
    }
    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, Error> {
        let trace_id = TraceId::new();
        let mut request = self
            .network
            .request(
                method.parse().expect("constant HTTP method"),
                &format!("{}{path}", self.base_url),
            )
            .bearer_auth(self.token.as_str())
            .header("x-nous-trace-id", trace_id.as_str());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self
            .network
            .send(request)
            .await
            .map_err(|error| Error::dependency(error).with_trace(trace_id.clone()))?;
        let trace_id = response
            .headers()
            .get("x-nous-trace-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<TraceId>().ok())
            .unwrap_or(trace_id);
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| Error::dependency(error).with_trace(trace_id.clone()))?;
        let value: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"message":text}));
        if !status.is_success() {
            let mut error: Error = value
                .get("error")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_else(|| {
                    Error::new(
                        ErrorCode::DependencyUnavailable,
                        format!("service returned {status}: {value}"),
                    )
                });
            if error.trace_id.is_none() {
                error = error.with_trace(trace_id);
            }
            return Err(error);
        }
        Ok(value)
    }
}
