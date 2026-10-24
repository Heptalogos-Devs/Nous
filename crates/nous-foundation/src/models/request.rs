//! Shared physical-input admission and SDK conversion for preview and execution.
use super::execution::PreparedRequest;
use super::{
    MediaData, MessagePart, MessageRole, ModelCapability, ModelInput, ModelModality, ModelProtocol,
    OutputContract, ResolvedRoute,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use genai::{
    chat::{ChatMessage, ChatOptions, ChatRequest, ContentPart, JsonSpec, MessageContent},
    embed::{EmbedOptions, EmbedRequest},
};
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
) -> Result<(PreparedRequest, ChannelAccess)> {
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
            let mut wire = Vec::new();
            for message in messages {
                if message.parts.is_empty() || message.parts.len() > 128 {
                    return Err(Error::invalid("message requires 1..128 content parts"));
                }
                let mut parts = Vec::new();
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
                            parts.push(ContentPart::from_text(text));
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
                                if !route.input_modalities.contains(&ModelModality::Audio) {
                                    supplied.audio = ChannelMode::Unavailable;
                                }
                            }
                            supplied.source_text = ChannelMode::Unavailable;
                            merge(&mut access, supplied);
                            let part = match data {
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
                                    let sdk_mime = match mime.as_str() {
                                        "audio/x-wav" => "audio/wav",
                                        "audio/mp4" => "audio/m4a",
                                        _ => &mime,
                                    };
                                    ContentPart::from_binary_base64(sdk_mime, data.as_str(), None)
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
                                    ContentPart::from_binary_url(&mime, url, None)
                                }
                            };
                            parts.push(part);
                        }
                    }
                }
                let content = MessageContent::from_parts(parts);
                wire.push(match message.role {
                    MessageRole::System => ChatMessage::system(content),
                    MessageRole::User => ChatMessage::user(content),
                    MessageRole::Assistant => ChatMessage::assistant(content),
                });
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
                PreparedRequest::Chat {
                    request: ChatRequest::new(wire),
                    options: Box::new(options),
                },
                access,
            ))
        }
        (ModelInput::Embedding { texts }, ModelProtocol::OpenaiEmbeddings) => {
            if texts.is_empty()
                || texts.len() > route.parameters.max_batch_size
                || texts.iter().any(|text| text.trim().is_empty())
                || texts.iter().map(String::len).sum::<usize>() > route.parameters.max_input_bytes
            {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "embedding input exceeds route batch or byte limits",
                ));
            }
            Ok((
                PreparedRequest::Embedding {
                    request: EmbedRequest::new_batch(texts.clone()),
                    options: Box::new(
                        EmbedOptions::new()
                            .with_encoding_format("float")
                            .with_capture_raw_body(true),
                    ),
                    contract: super::openai_embedding::ResponseContract {
                        space: route.space()?,
                        inputs: texts.len(),
                        max_bytes: route.parameters.max_response_bytes,
                    },
                },
                no_access(),
            ))
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
fn merge(target: &mut ChannelAccess, source: ChannelAccess) {
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
