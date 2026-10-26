use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use nous_domain::{CallContext, Error, Result, SourceReadScope};

pub(crate) const HEADER: &str = "x-nous-source-read";
pub(crate) fn encode(scopes: &[SourceReadScope]) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(scopes)?))
}
pub(crate) fn from_value(value: &serde_json::Value) -> Result<Vec<SourceReadScope>> {
    validate(serde_json::from_value(value.clone()).map_err(|error| {
        Error::invalid("source-read metadata must be a JSON list of SourceReadScope")
            .with_cause(&error)
    })?)
}
pub(super) fn bind_headers(
    mut context: CallContext,
    headers: &axum::http::HeaderMap,
) -> Result<CallContext> {
    for header in headers.get_all(HEADER).iter() {
        let text = header.to_str().map_err(|error| {
            Error::invalid("source-read header must be base64url UTF8 JSON").with_cause(&error)
        })?;
        let bytes = URL_SAFE_NO_PAD.decode(text).map_err(|error| {
            Error::invalid("source-read header requires unpadded base64url").with_cause(&error)
        })?;
        let scopes = validate(serde_json::from_slice(&bytes).map_err(|error| {
            Error::invalid("source-read header must contain a JSON list of SourceReadScope")
                .with_cause(&error)
        })?)?;
        for scope in scopes {
            context = context.with_source_read(scope)?;
        }
    }
    Ok(context)
}
fn validate(scopes: Vec<SourceReadScope>) -> Result<Vec<SourceReadScope>> {
    for scope in &scopes {
        scope.validate()?;
    }
    Ok(scopes)
}
