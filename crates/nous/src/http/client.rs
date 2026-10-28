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
    language: Option<String>,
    source_read: Vec<nous_domain::SourceReadScope>,
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
            language: None,
            source_read: Vec::new(),
        })
    }
    pub fn with_language(&self, language: &str) -> Self {
        Self {
            language: Some(language.to_owned()),
            ..self.clone()
        }
    }
    pub fn with_source_read(&self, scope: nous_domain::SourceReadScope) -> Result<Self, Error> {
        scope.validate()?;
        let mut client = self.clone();
        let scope = scope.canonical();
        if !client.source_read.contains(&scope) {
            client.source_read.push(scope);
        }
        Ok(client)
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
    pub async fn upload_material<R: tokio::io::AsyncRead + Send + Sync + 'static>(
        &self,
        operation_id: String,
        input: nous_domain::media::PutMediaRequest,
        reader: R,
    ) -> Result<Value, Error> {
        input.validate()?;
        let query = super::materials::UploadQuery {
            operation_id,
            namespace_id: input.namespace_id,
            owner_subject_id: input.owner_subject_id,
            source: input.source,
            event_time: input
                .event_time
                .map(|time| serde_json::to_string(&time))
                .transpose()?,
            expected_content_hash: input.expected_content_hash,
        };
        let trace_id = TraceId::new();
        let mut request = self
            .authorized_request("POST", "/api/media/materials", &trace_id)?
            .query(&query)
            .header(reqwest::header::CONTENT_TYPE, input.media_type)
            .body(reqwest::Body::wrap_stream(
                tokio_util::io::ReaderStream::new(reader),
            ));
        if let Some(length) = input.declared_byte_len {
            request = request.header(reqwest::header::CONTENT_LENGTH, length);
        }
        let response = self.send(request, &trace_id).await?;
        Self::decode(response, trace_id).await
    }

    pub async fn download_material(
        &self,
        id: &nous_domain::MaterialId,
    ) -> Result<reqwest::Response, Error> {
        let trace_id = TraceId::new();
        let request =
            self.authorized_request("GET", &format!("/api/materials/{id}/content"), &trace_id)?;
        let response = self.send(request, &trace_id).await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Self::decode(response, trace_id).await?;
            Err(Error::internal("unsuccessful download lacked an error"))
        }
    }

    fn authorized_request(
        &self,
        method: &str,
        path: &str,
        trace_id: &TraceId,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let request = self
            .network
            .request(
                method.parse().expect("constant HTTP method"),
                &format!("{}{path}", self.base_url),
            )
            .bearer_auth(self.token.as_str())
            .header("x-nous-trace-id", trace_id.as_str());
        let request = match &self.language {
            Some(language) => request.header(reqwest::header::ACCEPT_LANGUAGE, language),
            None => request,
        };
        Ok(if self.source_read.is_empty() {
            request
        } else {
            request.header(
                super::source_read::HEADER,
                super::source_read::encode(&self.source_read)?,
            )
        })
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
        trace_id: &TraceId,
    ) -> Result<reqwest::Response, Error> {
        self.network
            .send(request)
            .await
            .map_err(|error| Error::dependency(error.without_url()).with_trace(trace_id.clone()))
    }

    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, Error> {
        let trace_id = TraceId::new();
        let mut request = self.authorized_request(method, path, &trace_id)?;
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = self.send(request, &trace_id).await?;
        Self::decode(response, trace_id).await
    }

    async fn decode(response: reqwest::Response, trace_id: TraceId) -> Result<Value, Error> {
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
