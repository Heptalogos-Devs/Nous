use super::{Media, MediaPolicy, PreparationRequest, PreparedItem, PreparedMedia};
use crate::execution;
use nous_domain::{
    CallContext, Error, Result,
    media::{DerivationQuality, MediaDelivery, MediaMetadata, SourceCoordinate},
};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

impl Media {
    pub async fn prepare(
        &self,
        ctx: &CallContext,
        request: PreparationRequest,
        cancel: &CancellationToken,
    ) -> Result<PreparedMedia> {
        request.blob.validate()?;
        request.metadata.validate(&request.blob.media_type)?;
        request
            .coordinate
            .validate_bounds(request.blob.byte_len, &request.metadata)?;
        let service = self.clone();
        let ctx = ctx.clone();
        execution::owned_async(cancel, move |local| async move {
            let policy = service.policy().await?;
            if matches!(request.delivery, MediaDelivery::SampledFrames) {
                return service.sample(&ctx, &request, &policy, &local).await;
            }
            let animated = matches!(request.metadata, MediaMetadata::Image { frames: 2.., .. });
            let (mut item, version) = if matches!(request.coordinate, SourceCoordinate::Whole)
                && !animated
            {
                (
                    PreparedItem {
                        blob: request.blob.clone(),
                        object: None,
                        metadata: request.metadata.clone(),
                        coordinate: SourceCoordinate::Whole,
                        at_ms: None,
                    },
                    String::new(),
                )
            } else {
                let version = match request.coordinate {
                    SourceCoordinate::ByteRange { .. } | SourceCoordinate::TextSpan { .. } => {
                        String::new()
                    }
                    SourceCoordinate::Page { .. } => {
                        service.document_version(&ctx, &policy, &local).await?
                    }
                    _ => service.ffmpeg_version(&ctx, &policy, &local).await?,
                };
                let item = if matches!(request.coordinate, SourceCoordinate::Whole) && animated {
                    service.animate(&ctx, &request, &policy, &local).await?
                } else {
                    service.crop(&ctx, &request, &policy, &local).await?
                };
                (item, version)
            };
            if matches!(item.metadata, MediaMetadata::Image { frames: 2.., .. }) {
                let selected = PreparationRequest {
                    blob: item.blob.clone(),
                    metadata: item.metadata.clone(),
                    coordinate: SourceCoordinate::Whole,
                    delivery: MediaDelivery::Native,
                };
                let mut video = service.animate(&ctx, &selected, &policy, &local).await?;
                video.coordinate = item.coordinate.clone();
                item = video;
            }
            item.coordinate
                .validate_bounds(request.blob.byte_len, &request.metadata)?;
            if matches!(request.coordinate, SourceCoordinate::VideoFrame { .. })
                && !request.coordinate.contains(&item.coordinate)
            {
                return Err(Error::invalid(
                    "decoded frame timestamp differs from the fixed source frame locator",
                ));
            }
            let source_duration = match request.coordinate {
                SourceCoordinate::TimeSpan { start_ms, end_ms } => Some(end_ms - start_ms),
                _ => request.metadata.duration_ms(),
            };
            let shortened = source_duration
                .zip(item.metadata.duration_ms())
                .is_some_and(|(source, output)| output.saturating_add(2) < source);
            if matches!(
                request.coordinate,
                SourceCoordinate::Whole | SourceCoordinate::TimeSpan { .. }
            ) && super::transform::has_audio(&request.metadata)
                && !super::transform::has_audio(&item.metadata)
            {
                return Err(Error::invalid(
                    "native preparation lost the source audio track",
                ));
            }
            let items = vec![item];
            let preprocessing_digest = digest(&request, &policy, &version, &items)?;
            Ok(PreparedMedia {
                items,
                quality: DerivationQuality {
                    partial: shortened,
                    source_duration_ms: source_duration,
                    sampling: None,
                },
                preprocessing_digest,
            })
        })
        .await
    }
}

pub(super) fn digest(
    request: &PreparationRequest,
    policy: &MediaPolicy,
    version: &str,
    items: &[PreparedItem],
) -> Result<String> {
    let outputs = items
        .iter()
        .map(|item| (&item.blob, &item.metadata, &item.coordinate, item.at_ms))
        .collect::<Vec<_>>();
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(request, policy, version, outputs))?)
    ))
}
