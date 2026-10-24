//! Tokio byte streams verify complete objects at EOF and enforce exact byte ranges.
use super::{ReferenceLease, validate_digest};
use crate::execution;
use nous_domain::{Error, Result};
use sha2::{Digest, Sha256};
use std::{
    io,
    ops::Range,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, ReadBuf, Take};
use tokio_util::sync::CancellationToken;

pub struct ObjectReader {
    file: Take<tokio::fs::File>,
    hasher: Option<Sha256>,
    expected_hash: String,
    expected_len: u64,
    read: u64,
    verified: bool,
    cancel: CancellationToken,
    _lease: Option<ReferenceLease>,
}

impl ObjectReader {
    pub(super) async fn open(
        source: &Path,
        hash: &str,
        byte_len: i64,
        range: Option<Range<u64>>,
        cancel: CancellationToken,
        lease: Option<ReferenceLease>,
    ) -> Result<Self> {
        validate_digest(hash)?;
        let byte_len = u64::try_from(byte_len)
            .map_err(|_| Error::invalid("object length must be nonnegative"))?;
        execution::check_cancelled(&cancel)?;
        if !tokio::fs::metadata(source).await?.is_file() {
            return Err(Error::invalid("content source must be a regular file"));
        }
        let mut file = tokio::fs::File::open(source).await?;
        if file.metadata().await?.len() != byte_len {
            return Err(Error::new(
                nous_domain::ErrorCode::DependencyUnavailable,
                "object length differs from authority",
            ));
        }
        let full = range.is_none();
        let expected_len = match range {
            Some(range) => {
                if range.start > range.end || range.end > byte_len {
                    return Err(Error::invalid(
                        "object range is outside its authority length",
                    ));
                }
                file.seek(io::SeekFrom::Start(range.start)).await?;
                range.end - range.start
            }
            None => byte_len,
        };
        Ok(Self {
            file: file.take(expected_len + u64::from(full)),
            hasher: full.then(Sha256::new),
            expected_hash: hash.into(),
            expected_len,
            read: 0,
            verified: false,
            cancel,
            _lease: lease,
        })
    }
}

impl AsyncRead for ObjectReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Err(error) = execution::check_cancelled(&self.cancel) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::Interrupted, error)));
        }
        if buffer.remaining() == 0 || self.verified {
            return Poll::Ready(Ok(()));
        }
        let before = buffer.filled().len();
        match Pin::new(&mut self.file).poll_read(cx, buffer) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {}
        }
        let bytes = &buffer.filled()[before..];
        self.read += bytes.len() as u64;
        if self.read > self.expected_len {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "object exceeds its authority length",
            )));
        }
        if let Some(hasher) = &mut self.hasher {
            hasher.update(bytes);
        }
        if bytes.is_empty() {
            if self.read != self.expected_len {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "object is shorter than its authority length",
                )));
            }
            if let Some(hasher) = self.hasher.take()
                && format!("{:x}", hasher.finalize()) != self.expected_hash
            {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "object content hash differs from authority",
                )));
            }
            self.verified = true;
        }
        Poll::Ready(Ok(()))
    }
}
