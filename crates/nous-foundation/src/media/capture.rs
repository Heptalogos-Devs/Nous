use super::{CapturedMedia, Media, text};
use crate::execution;
use nous_domain::{
    CallContext, Error, Result,
    media::{BlobReference, MediaMetadata, canonical_media_type},
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

impl Media {
    pub async fn capture_text(
        &self,
        text: &str,
        cancel: &CancellationToken,
    ) -> Result<CapturedMedia> {
        let policy = self.policy().await?;
        let max = policy
            .max_buffered_text_bytes
            .min(policy.max_original_bytes);
        self.publish_text(text, max, cancel).await
    }
    pub(super) async fn publish_text(
        &self,
        text: &str,
        max: u64,
        cancel: &CancellationToken,
    ) -> Result<CapturedMedia> {
        if text.is_empty() {
            return Err(Error::invalid("text media must contain UTF-8 bytes"));
        }
        let object = self
            .objects
            .publish_bytes(text.as_bytes(), max, cancel)
            .await?;
        Ok(CapturedMedia {
            object,
            media_type: "text/plain".into(),
            metadata: MediaMetadata::Text,
        })
    }
    pub async fn capture<R: AsyncRead + Unpin + Send + 'static>(
        &self,
        ctx: &CallContext,
        reader: R,
        declared_mime: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<CapturedMedia> {
        let declared_mime = declared_mime
            .map(|mime| canonical_media_type(&mime))
            .transpose()?;
        let service = self.clone();
        let ctx = ctx.clone();
        execution::owned_async(cancel, move |local| async move {
            let policy = service.policy().await?;
            let object = service
                .objects
                .publish(reader, policy.max_original_bytes, &local)
                .await?;
            let (media_type, metadata) = service
                .inspect(&ctx, &object, declared_mime, &policy, &local)
                .await?;
            Ok(CapturedMedia {
                object,
                media_type,
                metadata,
            })
        })
        .await
    }
    pub(super) async fn inspect(
        &self,
        ctx: &CallContext,
        object: &crate::content::PublishedObject,
        declared_mime: Option<String>,
        policy: &super::MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<(String, MediaMetadata)> {
        if object.byte_len == 0 {
            return Err(Error::invalid("media must contain bytes"));
        }
        let mut original = self
            .objects
            .open(&object.digest, object.byte_len, cancel)
            .await?;
        let mut prefix = Vec::new();
        (&mut original).take(8192).read_to_end(&mut prefix).await?;
        drop(original);
        let magic = infer::get(&prefix).map(|kind| kind.mime_type().to_owned());
        let mut media_type = match magic {
            Some(mime) => mime,
            None => {
                let original = self
                    .objects
                    .open(&object.digest, object.byte_len, cancel)
                    .await?;
                if text::is_utf8(original).await? {
                    "text/plain; charset=utf-8".into()
                } else {
                    declared_mime
                        .filter(|mime| {
                            ["image/", "audio/", "video/"]
                                .iter()
                                .any(|prefix| mime.starts_with(prefix))
                        })
                        .unwrap_or_else(|| "application/octet-stream".into())
                }
            }
        };
        let blob = BlobReference {
            content_hash: object.digest.clone(),
            byte_len: object.byte_len as u64,
            media_type: media_type.clone(),
        };
        let metadata = if media_type.starts_with("text/") {
            MediaMetadata::Text
        } else if media_type == "application/pdf" {
            self.document_metadata(ctx, &blob, policy, cancel).await?
        } else if ["image/", "audio/", "video/"]
            .iter()
            .any(|prefix| media_type.starts_with(prefix))
        {
            let (actual, metadata) = self.probe_blob(ctx, &blob, policy, cancel).await?;
            media_type = actual;
            metadata
        } else {
            MediaMetadata::Binary
        };
        metadata.validate(&media_type)?;
        Ok((media_type, metadata))
    }
}
