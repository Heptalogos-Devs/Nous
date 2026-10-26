use super::{
    ModelCapability, ModelMediaReader, ModelModality, ModelProtocol, ResolvedRoute,
    embedding::{PreparedEmbeddingRequest, ResponseContract},
    execution::PreparedRequest,
    request,
};
use nous_domain::{
    ConfigScope, EmbeddingPart, Error, ErrorCode, Result,
    media::{ChannelAccess, ChannelMode, canonical_media_type},
};
use tokio_util::sync::CancellationToken;

pub(super) struct ValidatedEmbedding {
    inputs: Vec<Vec<EmbeddingPart>>,
    route: ResolvedRoute,
    contract: ResponseContract,
}
pub(super) fn validate(
    route: &ResolvedRoute,
    inputs: &[Vec<EmbeddingPart>],
) -> Result<(ValidatedEmbedding, ChannelAccess)> {
    if !route.capabilities.contains(&ModelCapability::Embedding) {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "model does not declare embedding output",
        ));
    }
    if inputs.is_empty()
        || inputs.len() > route.parameters.max_batch_size
        || (route.resource.protocol == ModelProtocol::ArkMultimodalEmbeddings && inputs.len() != 1)
    {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "embedding input exceeds the selected protocol's batch capacity",
        ));
    }
    let profile = route
        .embedding
        .as_ref()
        .ok_or_else(|| Error::invalid("embedding route has no immutable space profile"))?;
    profile.validate()?;
    let mut text_bytes = serde_json::to_vec(&profile.parameters)?.len();
    let mut media_bytes = 0usize;
    let mut access = request::no_access();
    for parts in inputs {
        if parts.is_empty() || parts.len() > 64 {
            return Err(Error::invalid(
                "one embedding item requires 1..64 joint content parts",
            ));
        }
        if route.resource.protocol == ModelProtocol::OpenaiEmbeddings
            && !matches!(parts.as_slice(), [EmbeddingPart::Text { .. }])
        {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "OpenAI text embeddings consume exactly one text part per item",
            ));
        }
        for part in parts {
            match part {
                EmbeddingPart::Text { text, source } => {
                    if !route.input_modalities.contains(&ModelModality::Text) {
                        return Err(Error::new(
                            ErrorCode::CapabilityUnavailable,
                            "model does not declare text input",
                        ));
                    }
                    if text.trim().is_empty() || text.contains('\0') {
                        return Err(Error::invalid(
                            "embedding text must be nonempty and contain no NUL",
                        ));
                    }
                    text_bytes = text_bytes.saturating_add(text.len());
                    let mode = match source {
                        Some(
                            nous_domain::media::MediaReference::Representation { .. }
                            | nous_domain::media::MediaReference::DerivedRegion { .. },
                        ) => ChannelMode::Representation,
                        _ => ChannelMode::Original,
                    };
                    request::merge(
                        &mut access,
                        ChannelAccess {
                            visual: ChannelMode::Unavailable,
                            audio: ChannelMode::Unavailable,
                            source_text: mode,
                        },
                    );
                }
                EmbeddingPart::Media {
                    source,
                    blob,
                    channels,
                } => {
                    source.validate()?;
                    blob.validate()?;
                    let mime = canonical_media_type(&blob.media_type)?;
                    let modality = if mime.starts_with("image/") {
                        ModelModality::Image
                    } else if mime.starts_with("video/") {
                        ModelModality::Video
                    } else {
                        return Err(Error::new(
                            ErrorCode::CapabilityUnavailable,
                            "Ark embedding accepts native image or visual video input",
                        ));
                    };
                    if !route.input_modalities.contains(&modality) {
                        return Err(Error::new(
                            ErrorCode::CapabilityUnavailable,
                            "embedding model does not declare this media modality",
                        ));
                    }
                    if channels.visual != ChannelMode::Original
                        || channels.audio != ChannelMode::Unavailable
                        || channels.source_text != ChannelMode::Unavailable
                    {
                        return Err(Error::new(
                            ErrorCode::CapabilityUnavailable,
                            "visual embedding requires exact visual delivery; audio and source-text channels must be absent",
                        ));
                    }
                    media_bytes = media_bytes.saturating_add(
                        usize::try_from(blob.byte_len)
                            .map_err(|_| Error::invalid("media length exceeds platform range"))?,
                    );
                    request::merge(&mut access, *channels);
                }
            }
        }
    }
    if text_bytes > route.parameters.max_input_bytes
        || media_bytes > route.parameters.max_media_bytes
    {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "embedding text or media exceeds the route byte limits",
        ));
    }
    Ok((
        ValidatedEmbedding {
            inputs: inputs.to_vec(),
            route: route.clone(),
            contract: ResponseContract {
                space: route.space()?,
                inputs: inputs.len(),
                max_bytes: route.parameters.max_response_bytes,
            },
        },
        access,
    ))
}
impl ValidatedEmbedding {
    pub(super) async fn encode(
        self,
        scope: &ConfigScope,
        reader: &dyn ModelMediaReader,
        cancel: &CancellationToken,
    ) -> Result<PreparedRequest> {
        let request = match self.route.resource.protocol {
            ModelProtocol::OpenaiEmbeddings => {
                let mut text = Vec::new();
                for parts in self.inputs {
                    let [EmbeddingPart::Text { text: value, .. }] = parts.as_slice() else {
                        return Err(Error::invalid(
                            "OpenAI embedding input is not a single text part",
                        ));
                    };
                    text.push(value.clone());
                }
                PreparedEmbeddingRequest::Openai(super::openai_embedding::Request {
                    model: self.route.resource.model.clone(),
                    input: text,
                    dimensions: self.contract.space.dimension,
                    encoding_format: "float",
                    parameters: self.contract.space.parameters.clone(),
                })
            }
            ModelProtocol::ArkMultimodalEmbeddings => PreparedEmbeddingRequest::Ark(
                super::ark_embedding::encode(self.inputs, &self.route, scope, reader, cancel)
                    .await?,
            ),
            _ => {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "selected protocol does not encode embeddings",
                ));
            }
        };
        Ok(PreparedRequest::Embedding {
            request,
            contract: self.contract,
        })
    }
}
