//! An invocation keeps immutable locators; the source owner admits actual bytes.
use super::{MediaData, MessagePart, MessageRole, ModelMessage};
use base64::{Engine, engine::general_purpose::STANDARD};
use genai::chat::{ChatMessage, ChatRequest, ContentPart, MessageContent};
use nous_domain::{
    ConfigScope, Error, ErrorCode, Result,
    media::{BlobReference, MediaReference},
};
use std::{future::Future, pin::Pin};
use tokio_util::sync::CancellationToken;

pub type MediaReadFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'static>>;
pub trait ModelMediaReader: Send + Sync {
    /// Resolve the exact locator under this scope and verify its immutable blob identity.
    fn read(
        &self,
        scope: ConfigScope,
        source: MediaReference,
        expected: BlobReference,
        max_bytes: usize,
        cancel: CancellationToken,
    ) -> MediaReadFuture;
}

pub(super) async fn encode(
    messages: Vec<ModelMessage>,
    scope: &ConfigScope,
    reader: &dyn ModelMediaReader,
    max_bytes: usize,
    cancel: &CancellationToken,
) -> Result<ChatRequest> {
    let mut wire = Vec::with_capacity(messages.len());
    for message in messages {
        crate::execution::check_cancelled(cancel)?;
        let mut parts = Vec::with_capacity(message.parts.len());
        for part in message.parts {
            parts.push(match part {
                MessagePart::Text { text, .. } => ContentPart::from_text(text),
                MessagePart::Media {
                    media_type,
                    data,
                    source,
                } => {
                    let media_type = nous_domain::media::canonical_media_type(&media_type)?;
                    let mime = match media_type.as_str() {
                        "audio/x-wav" => "audio/wav",
                        "audio/mp4" => "audio/m4a",
                        _ => &media_type,
                    };
                    match data {
                        MediaData::Base64 { data } => {
                            ContentPart::from_binary_base64(mime, data, None)
                        }
                        MediaData::Url { url } => ContentPart::from_binary_url(mime, url, None),
                        MediaData::Blob { blob } => {
                            let source = source.ok_or_else(|| {
                                Error::invalid(
                                    "stored model media requires an exact source locator",
                                )
                            })?;
                            let bytes = reader
                                .read(
                                    scope.clone(),
                                    source.reference,
                                    blob.clone(),
                                    max_bytes,
                                    cancel.clone(),
                                )
                                .await?;
                            if bytes.len() as u64 != blob.byte_len || bytes.len() > max_bytes {
                                return Err(Error::new(
                                    ErrorCode::Conflict,
                                    "source media bytes differ from the admitted blob length",
                                ));
                            }
                            ContentPart::from_binary_base64(mime, STANDARD.encode(bytes), None)
                        }
                    }
                }
            });
        }
        let content = MessageContent::from_parts(parts);
        wire.push(match message.role {
            MessageRole::System => ChatMessage::system(content),
            MessageRole::User => ChatMessage::user(content),
            MessageRole::Assistant => ChatMessage::assistant(content),
        });
    }
    Ok(ChatRequest::new(wire))
}
