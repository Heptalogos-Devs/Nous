use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub(super) async fn capture<R: AsyncRead + Unpin>(
    mut reader: Option<R>,
    limit: usize,
    total: Arc<AtomicUsize>,
    overflow: CancellationToken,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(reader) = reader.as_mut() {
        let mut buffer = [0u8; 8192];
        loop {
            let size = reader.read(&mut buffer).await?;
            if size == 0 {
                break;
            }
            let before = total
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                    Some(used.saturating_add(size))
                })
                .expect("counter update always succeeds");
            let keep = size.min(limit.saturating_sub(before));
            bytes.extend_from_slice(&buffer[..keep]);
            if keep < size {
                overflow.cancel();
            }
        }
    }
    Ok(bytes)
}

pub(super) async fn input<W: AsyncWrite + Unpin>(
    writer: Option<W>,
    bytes: Option<Vec<u8>>,
) -> std::io::Result<()> {
    if let Some(mut writer) = writer {
        if let Some(bytes) = bytes
            && let Err(error) = writer.write_all(&bytes).await
            && error.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(error);
        }
        let _ = writer.shutdown().await;
    }
    Ok(())
}
