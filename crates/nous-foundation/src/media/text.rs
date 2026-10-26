use super::Media;
use crate::{content::ObjectReader, execution};
use nous_domain::{Error, ErrorCode, Result, media::BlobReference};
use std::ops::Range;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

impl Media {
    pub async fn open(
        &self,
        blob: &BlobReference,
        cancel: &CancellationToken,
    ) -> Result<ObjectReader> {
        blob.validate()?;
        self.objects
            .open(&blob.content_hash, blob.byte_len as i64, cancel)
            .await
    }
    pub async fn read_text(
        &self,
        blob: &BlobReference,
        cancel: &CancellationToken,
    ) -> Result<String> {
        self.read_text_selected(blob, None, cancel).await
    }
    pub async fn read_text_range(
        &self,
        blob: &BlobReference,
        range: Range<u64>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        if range.start >= range.end || range.end > blob.byte_len {
            return Err(Error::invalid("text range is outside the blob"));
        }
        self.read_text_selected(blob, Some(range), cancel).await
    }
    async fn read_text_selected(
        &self,
        blob: &BlobReference,
        range: Option<Range<u64>>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        blob.validate()?;
        let max = self.policy().await?.max_buffered_text_bytes;
        let length = range
            .as_ref()
            .map_or(blob.byte_len, |range| range.end - range.start);
        if length > max {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "selected text exceeds the media buffer policy",
            ));
        }
        let service = self.clone();
        let blob = blob.clone();
        execution::owned_async(cancel, move |local| async move {
            let reader = match range {
                Some(range) => {
                    service
                        .objects
                        .read_range(&blob.content_hash, blob.byte_len as i64, range, &local)
                        .await?
                }
                None => service.open(&blob, &local).await?,
            };
            decode(reader, max).await
        })
        .await
    }
}
async fn decode(reader: impl AsyncRead + Unpin, max: u64) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > max {
        return Err(Error::new(
            ErrorCode::ResourceExhausted,
            "text stream exceeds the media buffer policy",
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        Error::invalid("selected media is not complete UTF-8 text").with_cause(&error)
    })
}

pub(super) async fn is_utf8(mut reader: ObjectReader) -> Result<bool> {
    let mut tail = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let size = reader.read(&mut buffer).await?;
        if size == 0 {
            return Ok(tail.is_empty());
        }
        tail.extend_from_slice(&buffer[..size]);
        match std::str::from_utf8(&tail) {
            Ok(_) => tail.clear(),
            Err(error) if error.error_len().is_some() => return Ok(false),
            Err(error) => {
                tail = tail[error.valid_up_to()..].to_vec();
            }
        }
    }
}
