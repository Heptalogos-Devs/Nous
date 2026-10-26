use super::{
    ModelCallRecord, ModelFailure, ModelMediaReader,
    embedding::{ResponseContract, invalid, normalize, validate_model},
};
use crate::network::NetworkClients;
use base64::{Engine, engine::general_purpose::STANDARD};
use genai::ServiceTarget;
use nous_domain::{ConfigScope, EmbeddingPart, Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
pub(super) struct Request {
    model: String,
    input: Vec<Input>,
    dimensions: usize,
    encoding_format: &'static str,
    #[serde(flatten)]
    parameters: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Input {
    Text { text: String },
    ImageUrl { image_url: Url },
    VideoUrl { video_url: Url },
}
#[derive(Clone, Serialize)]
struct Url {
    url: String,
}
#[derive(Deserialize)]
struct Response {
    model: String,
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    embedding: Vec<f32>,
}

pub(super) async fn encode(
    inputs: Vec<Vec<EmbeddingPart>>,
    route: &super::ResolvedRoute,
    scope: &ConfigScope,
    reader: &dyn ModelMediaReader,
    cancel: &CancellationToken,
) -> Result<Request> {
    let [parts] = inputs.as_slice() else {
        return Err(Error::invalid(
            "Ark multimodal embedding requires exactly one joint input item",
        ));
    };
    let mut wire = Vec::new();
    let mut remaining = route.parameters.max_media_bytes;
    for part in parts {
        crate::execution::check_cancelled(cancel)?;
        wire.push(match part {
            EmbeddingPart::Text { text, .. } => Input::Text { text: text.clone() },
            EmbeddingPart::Media { source, blob, .. } => {
                let bytes = super::media_input::load_blob(
                    scope,
                    source.clone(),
                    blob.clone(),
                    reader,
                    remaining,
                    cancel,
                )
                .await?;
                remaining = remaining.checked_sub(bytes.len()).ok_or_else(|| {
                    Error::invalid("embedding media exceeds its frozen byte limit")
                })?;
                let mime = nous_domain::media::canonical_media_type(&blob.media_type)?;
                let url = Url {
                    url: format!("data:{mime};base64,{}", STANDARD.encode(bytes)),
                };
                if mime.starts_with("image/") {
                    Input::ImageUrl { image_url: url }
                } else {
                    Input::VideoUrl { video_url: url }
                }
            }
        });
    }
    let profile = route
        .embedding
        .as_ref()
        .ok_or_else(|| Error::invalid("Ark route has no embedding space profile"))?;
    Ok(Request {
        model: route.resource.model.clone(),
        input: wire,
        dimensions: profile.dimension,
        encoding_format: "float",
        parameters: profile.parameters.clone(),
    })
}
pub(super) async fn execute<M>(
    network: &NetworkClients,
    target: ServiceTarget,
    request: Request,
    record: &mut ModelCallRecord<M>,
    contract: &ResponseContract,
) -> std::result::Result<Vec<Vec<f32>>, ModelFailure> {
    let body = super::embedding_http::exchange(
        network,
        target,
        "embeddings/multimodal",
        &request,
        record,
        contract.max_bytes,
    )
    .await?;
    let response: Response = serde_json::from_value(body).map_err(|error| {
        invalid("Ark multimodal response requires one data.embedding object")
            .with_source(Box::new(error))
    })?;
    validate_model(&response.model, &contract.space)?;
    if contract.inputs != 1 {
        return Err(invalid(
            "Ark multimodal response belongs to one joint input item",
        ));
    }
    Ok(vec![normalize(response.data.embedding, &contract.space)?])
}
