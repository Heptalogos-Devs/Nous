//! Shared physical-input admission and SDK conversion for preview and execution.
use super::execution::PreparedRequest;
use super::{
    MediaData, MessagePart, MessageRole, ModelCapability, ModelInput, ModelModality, ModelProtocol,
    OutputContract, ResolvedRoute,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use genai::chat::{ChatOptions, JsonSpec};
use tokio_util::sync::CancellationToken;

pub(super) enum ValidatedRequest {
    Chat {
        messages: Vec<super::ModelMessage>,
        options: Box<ChatOptions>,
        max_media_bytes: usize,
    },
    Embedding(Box<super::embedding_request::ValidatedEmbedding>),
}
impl ValidatedRequest {
    pub async fn encode(
        self,
        scope: &nous_domain::ConfigScope,
        reader: &dyn super::ModelMediaReader,
        cancel: &CancellationToken,
    ) -> Result<PreparedRequest> {
        crate::execution::check_cancelled(cancel)?;
        Ok(match self {
            Self::Chat {
                messages,
                options,
                max_media_bytes,
            } => PreparedRequest::Chat {
                request: super::media_input::encode(
                    messages,
                    scope,
                    reader,
                    max_media_bytes,
                    cancel,
                )
                .await?,
                options,
            },
            Self::Embedding(request) => request.encode(scope, reader, cancel).await?,
        })
    }
}
use nous_domain::{
    Error, ErrorCode, Result,
    media::{ChannelAccess, ChannelMode, canonical_media_type},
};

pub(super) fn normalize(input: &mut ModelInput) {
    if let ModelInput::Chat {
        output: OutputContract::JsonSchema { schema, .. },
        ..
    } = input
    {
        *schema =
            genai::chat::sanitize_json_schema(schema, genai::chat::JsonSchemaDialect::OpenAiStrict);
    }
}
pub(super) fn invocation_digest(route: &ResolvedRoute, input: &ModelInput) -> Result<String> {
    use sha2::{Digest, Sha256};
    let controls = match input {
        ModelInput::Chat { .. } => {
            serde_json::json!({"max_output_tokens":route.parameters.max_output_tokens,"parameters":route.parameters.native_parameters})
        }
        ModelInput::Embedding { .. } => {
            serde_json::json!({"space":route.embedding.as_ref().map(|_|route.space()).transpose()?})
        }
    };
    let identity = serde_json::json!({"protocol":route.resource.protocol,"endpoint":route.resource.endpoint,"model":route.resource.model,"input":input,"controls":controls});
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&identity)?)
    ))
}
pub(super) fn no_access() -> ChannelAccess {
    ChannelAccess {
        visual: ChannelMode::Unavailable,
        audio: ChannelMode::Unavailable,
        source_text: ChannelMode::Unavailable,
    }
}
pub(super) fn prepare(
    route: &ResolvedRoute,
    input: &ModelInput,
) -> Result<(ValidatedRequest, ChannelAccess)> {
    match (input, route.resource.protocol) {
        (ModelInput::Chat { messages, output }, ModelProtocol::OpenaiChatCompletions) => {
            if !route
                .capabilities
                .contains(&ModelCapability::TextGeneration)
            {
                return Err(Error::new(
                    ErrorCode::CapabilityUnavailable,
                    "model does not declare text generation",
                ));
            }
            if messages.is_empty() || messages.len() > 128 {
                return Err(Error::invalid("chat requires 1..128 messages"));
            }
            let mut access = no_access();
            let mut text_bytes = 0usize;
            let mut media_bytes = 0usize;
            for message in messages {
                if message.parts.is_empty() || message.parts.len() > 128 {
                    return Err(Error::invalid("message requires 1..128 content parts"));
                }
                for part in &message.parts {
                    match part {
                        MessagePart::Text { text, source } => {
                            if !route.input_modalities.contains(&ModelModality::Text) {
                                return Err(Error::new(
                                    ErrorCode::CapabilityUnavailable,
                                    "model does not declare text input",
                                ));
                            }
                            text_bytes = text_bytes.saturating_add(text.len());
                            if let Some(source) = source {
                                source.reference.validate()?;
                                let mut supplied = source.channels;
                                supplied.visual = downgrade(supplied.visual);
                                supplied.audio = downgrade(supplied.audio);
                                merge(&mut access, supplied);
                            }
                        }
                        MessagePart::Media {
                            media_type,
                            data,
                            source,
                        } => {
                            if !matches!(message.role, MessageRole::User) {
                                return Err(Error::invalid(
                                    "this protocol consumes media only in user messages",
                                ));
                            }
                            let mime = canonical_media_type(media_type)?;
                            let modality = if mime.starts_with("image/") {
                                ModelModality::Image
                            } else if mime.starts_with("audio/") {
                                ModelModality::Audio
                            } else if mime.starts_with("video/") {
                                ModelModality::Video
                            } else {
                                return Err(Error::new(
                                    ErrorCode::CapabilityUnavailable,
                                    "media MIME type has no installed model adapter",
                                ));
                            };
                            if !route.input_modalities.contains(&modality) {
                                return Err(Error::new(
                                    ErrorCode::CapabilityUnavailable,
                                    "model does not declare support for supplied media modality",
                                ));
                            }
                            let mut supplied = source
                                .as_ref()
                                .map_or_else(no_access, |source| source.channels);
                            if let Some(source) = source {
                                source.reference.validate()?;
                            }
                            if matches!(modality, ModelModality::Image) {
                                supplied.audio = ChannelMode::Unavailable;
                                if source.is_none() {
                                    supplied.visual = ChannelMode::Original;
                                }
                            } else if matches!(modality, ModelModality::Audio) {
                                supplied.visual = ChannelMode::Unavailable;
                                if source.is_none() {
                                    supplied.audio = ChannelMode::Original;
                                }
                                if !matches!(
                                    mime.as_str(),
                                    "audio/mpeg"
                                        | "audio/mp3"
                                        | "audio/wav"
                                        | "audio/x-wav"
                                        | "audio/aac"
                                        | "audio/mp4"
                                        | "audio/m4a"
                                ) {
                                    return Err(Error::new(
                                        ErrorCode::CapabilityUnavailable,
                                        "audio MIME type is not supported by this protocol",
                                    ));
                                }
                            } else {
                                if source.is_none() {
                                    supplied.visual = ChannelMode::Original;
                                }
                                if !route.input_modalities.contains(&ModelModality::Audio)
                                    && supplied.audio != ChannelMode::Unavailable
                                {
                                    return Err(Error::new(
                                        ErrorCode::CapabilityUnavailable,
                                        "model route cannot consume the declared video audio channel",
                                    ));
                                }
                            }
                            supplied.source_text = ChannelMode::Unavailable;
                            merge(&mut access, supplied);
                            match data {
                                MediaData::Blob { blob } => {
                                    blob.validate()?;
                                    if source.is_none()
                                        || canonical_media_type(&blob.media_type)? != mime
                                    {
                                        return Err(Error::invalid(
                                            "stored media requires a matching MIME type and exact source locator",
                                        ));
                                    }
                                    let bytes = usize::try_from(blob.byte_len).map_err(|_| {
                                        Error::new(
                                            ErrorCode::ResourceExhausted,
                                            "media length exceeds the platform range",
                                        )
                                    })?;
                                    media_bytes = media_bytes.saturating_add(bytes);
                                }
                                MediaData::Base64 { data } => {
                                    if data.len()
                                        > route
                                            .parameters
                                            .max_media_bytes
                                            .saturating_mul(4)
                                            .div_ceil(3)
                                            + 4
                                    {
                                        return Err(Error::new(
                                            ErrorCode::ResourceExhausted,
                                            "encoded media exceeds route byte limit",
                                        ));
                                    }
                                    let bytes = STANDARD.decode(data).map_err(|_| {
                                        Error::invalid(
                                            "media content must be valid standard base64",
                                        )
                                    })?;
                                    if bytes.is_empty() {
                                        return Err(Error::invalid(
                                            "media content must be nonempty",
                                        ));
                                    }
                                    media_bytes = media_bytes.saturating_add(bytes.len());
                                }
                                MediaData::Url { url } => {
                                    if matches!(modality, ModelModality::Audio) {
                                        return Err(Error::new(
                                            ErrorCode::CapabilityUnavailable,
                                            "this protocol requires inline audio bytes",
                                        ));
                                    }
                                    let parsed = reqwest::Url::parse(url).map_err(|_| {
                                        Error::invalid("media URL must be absolute HTTP")
                                    })?;
                                    if !matches!(parsed.scheme(), "http" | "https")
                                        || parsed.host().is_none()
                                        || !parsed.username().is_empty()
                                        || parsed.password().is_some()
                                    {
                                        return Err(Error::invalid(
                                            "media URL must be HTTP without user credentials",
                                        ));
                                    }
                                    text_bytes = text_bytes.saturating_add(url.len());
                                }
                            };
                        }
                    }
                }
            }
            let mut options = ChatOptions::default()
                .with_max_tokens(route.parameters.max_output_tokens)
                .with_capture_raw_body(true);
            if let OutputContract::JsonSchema { name, schema } = output {
                if !route
                    .capabilities
                    .contains(&ModelCapability::StructuredOutput)
                {
                    return Err(Error::new(
                        ErrorCode::CapabilityUnavailable,
                        "model does not declare structured output",
                    ));
                }
                if name.trim().is_empty() || !schema.is_object() {
                    return Err(Error::invalid(
                        "JSON Schema output requires a name and schema object",
                    ));
                }
                text_bytes = text_bytes.saturating_add(serde_json::to_vec(schema)?.len());
                options = options.with_response_format(JsonSpec::new(name.clone(), schema.clone()));
            }
            if text_bytes > route.parameters.max_input_bytes
                || media_bytes > route.parameters.max_media_bytes
            {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "invocation text or media exceeds route byte limits",
                ));
            }
            if !route.parameters.native_parameters.is_empty() {
                options =
                    options.with_extra_body(serde_json::json!(route.parameters.native_parameters));
            }
            Ok((
                ValidatedRequest::Chat {
                    messages: messages.clone(),
                    max_media_bytes: route.parameters.max_media_bytes,
                    options: Box::new(options),
                },
                access,
            ))
        }
        (
            ModelInput::Embedding { inputs },
            ModelProtocol::OpenaiEmbeddings | ModelProtocol::ArkMultimodalEmbeddings,
        ) => {
            let (request, access) = super::embedding_request::validate(route, inputs)?;
            Ok((ValidatedRequest::Embedding(Box::new(request)), access))
        }
        _ => Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "selected protocol does not consume this invocation type",
        )),
    }
}
fn downgrade(mode: ChannelMode) -> ChannelMode {
    match mode {
        ChannelMode::Original => ChannelMode::Representation,
        mode => mode,
    }
}
pub(super) fn merge(target: &mut ChannelAccess, source: ChannelAccess) {
    let combine = |a, b| match (a, b) {
        (ChannelMode::Original, _) | (_, ChannelMode::Original) => ChannelMode::Original,
        (ChannelMode::Representation, _) | (_, ChannelMode::Representation) => {
            ChannelMode::Representation
        }
        _ => ChannelMode::Unavailable,
    };
    target.visual = combine(target.visual, source.visual);
    target.audio = combine(target.audio, source.audio);
    target.source_text = combine(target.source_text, source.source_text);
}
