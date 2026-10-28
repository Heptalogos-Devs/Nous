use super::{ApiError, ApiState};
use axum::{
    Json, Router,
    body::Body,
    extract::{Extension, Path, Query, State, rejection::QueryRejection},
    http::{HeaderMap, HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::TryStreamExt;
use nous_domain::{CallContext, Error, MaterialId, NamespaceId, SubjectId, media::PutMediaRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::io::{ReaderStream, StreamReader};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadQuery {
    pub operation_id: String,
    pub namespace_id: NamespaceId,
    pub owner_subject_id: Option<SubjectId>,
    pub source: Option<String>,
    pub event_time: Option<String>,
    pub expected_content_hash: Option<String>,
}

pub fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/media/materials", post(upload))
        .route("/api/materials/{material}/content", get(download))
}

async fn upload(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    query: Result<Query<UploadQuery>, QueryRejection>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<Value>, ApiError> {
    let Query(query) = query.map_err(|error| Error::invalid(error.body_text()))?;
    let media_type =
        header_text(&headers, header::CONTENT_TYPE)?.unwrap_or("application/octet-stream");
    let declared_byte_len = header_text(&headers, header::CONTENT_LENGTH)?
        .map(|value| {
            value
                .parse()
                .map_err(|_| Error::invalid("invalid Content-Length"))
        })
        .transpose()?;
    let request = PutMediaRequest {
        namespace_id: query.namespace_id,
        owner_subject_id: query.owner_subject_id,
        media_type: media_type.to_owned(),
        declared_byte_len,
        expected_content_hash: query.expected_content_hash,
        source: query.source,
        event_time: query
            .event_time
            .map(|time| serde_json::from_str(&time))
            .transpose()
            .map_err(Error::from)?,
    };
    let stream = body.into_data_stream().map_err(std::io::Error::other);
    let receipt = state
        .app
        .put_media(
            &ctx,
            &query.operation_id,
            request,
            StreamReader::new(stream),
            &state.shutdown.child_token(),
        )
        .await?;
    Ok(Json(serde_json::to_value(receipt).map_err(Error::from)?))
}

async fn download(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let id: MaterialId = id.parse()?;
    let original = state
        .app
        .open_material(&ctx, &id, &state.shutdown.child_token())
        .await?;
    let mut response = Body::from_stream(ReaderStream::new(original.reader)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, value(&original.material.media_type)?);
    headers.insert(
        header::CONTENT_LENGTH,
        value(&original.material.byte_len.to_string())?,
    );
    headers.insert(
        header::ETAG,
        value(&format!("\"{}\"", original.material.content_hash))?,
    );
    headers.insert(
        "x-nous-content-sha256",
        value(&original.material.content_hash)?,
    );
    headers.insert("x-nous-commit-seq", value(&original.watermark.to_string())?);
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Result<Option<&str>, Error> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map_err(|_| Error::invalid("invalid media header"))
        })
        .transpose()
}
fn value(text: &str) -> Result<HeaderValue, Error> {
    HeaderValue::from_str(text).map_err(|_| Error::internal("invalid stored media response header"))
}
