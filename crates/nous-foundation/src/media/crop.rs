use super::{Media, MediaPolicy, PreparationRequest, PreparedItem, transform};
use nous_domain::{
    CallContext, Error, Result,
    media::{MediaMetadata, SourceCoordinate},
};
use tokio_util::sync::CancellationToken;

impl Media {
    pub(super) async fn animate(
        &self,
        ctx: &CallContext,
        request: &PreparationRequest,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<PreparedItem> {
        let (workspace, input) = self.stage(request, cancel).await?;
        let output = workspace.path().join("animation.mp4");
        let mut args = transform::base_args(policy);
        args.extend([
            "-i".into(),
            input.into_os_string(),
            "-an".into(),
            "-vf".into(),
            "pad=ceil(iw/2)*2:ceil(ih/2)*2".into(),
            "-fps_mode".into(),
            "vfr".into(),
            "-c:v".into(),
            "libx264".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-threads".into(),
            policy.process_threads.to_string().into(),
            "-fs".into(),
            policy.max_derivative_bytes.to_string().into(),
            "-y".into(),
            output.clone().into_os_string(),
        ]);
        let result = self.ffmpeg(ctx, workspace, args, policy, cancel).await?;
        let item = self
            .publish_output(ctx, &output, SourceCoordinate::Whole, None, policy, cancel)
            .await?;
        drop(result);
        Ok(item)
    }
    pub(super) async fn crop(
        &self,
        ctx: &CallContext,
        request: &PreparationRequest,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<PreparedItem> {
        match &request.coordinate {
            SourceCoordinate::ByteRange { start, end }
            | SourceCoordinate::TextSpan { start, end } => {
                if matches!(request.metadata, MediaMetadata::Text) {
                    let text = self
                        .read_text_bounded(
                            &request.blob,
                            Some(*start..*end),
                            policy.max_buffered_text_bytes,
                            cancel,
                        )
                        .await?;
                    let captured = self
                        .publish_text(&text, policy.max_derivative_bytes, cancel)
                        .await?;
                    return Ok(PreparedItem {
                        blob: nous_domain::media::BlobReference {
                            content_hash: captured.object.digest.clone(),
                            media_type: captured.media_type,
                            byte_len: captured.object.byte_len as u64,
                        },
                        object: Some(captured.object),
                        metadata: captured.metadata,
                        coordinate: request.coordinate.clone(),
                        at_ms: None,
                    });
                }
                let object = {
                    let source = self
                        .objects
                        .read_range(
                            &request.blob.content_hash,
                            request.blob.byte_len as i64,
                            *start..*end,
                            cancel,
                        )
                        .await?;
                    self.objects
                        .publish(source, policy.max_derivative_bytes, cancel)
                        .await?
                };
                let (media_type, metadata) =
                    self.inspect(ctx, &object, None, policy, cancel).await?;
                return Ok(PreparedItem {
                    blob: nous_domain::media::BlobReference {
                        content_hash: object.digest.clone(),
                        media_type,
                        byte_len: object.byte_len as u64,
                    },
                    object: Some(object),
                    metadata,
                    coordinate: request.coordinate.clone(),
                    at_ms: None,
                });
            }
            SourceCoordinate::Page { .. } => {
                return self.render_page(ctx, request, policy, cancel).await;
            }
            _ => {}
        }
        let (workspace, input) = self.stage(request, cancel).await?;
        let mut args = transform::base_args(policy);
        let mut coordinate = request.coordinate.clone();
        let (extension, filter) = match &request.coordinate {
            SourceCoordinate::ImageRect { rect } => {
                args.extend(["-i".into(), input.into_os_string()]);
                if matches!(request.metadata, MediaMetadata::Video { .. }) {
                    args.extend([
                        "-map".into(),
                        "0:v:0".into(),
                        "-map".into(),
                        "0:a?".into(),
                        "-c:v".into(),
                        "ffv1".into(),
                        "-c:a".into(),
                        "pcm_s16le".into(),
                    ]);
                    ("mkv", Some(transform::visual_filter(Some(rect), None)))
                } else if matches!(request.metadata, MediaMetadata::Image { frames: 2.., .. }) {
                    args.push("-an".into());
                    ("gif", Some(transform::visual_filter(Some(rect), None)))
                } else {
                    args.extend(["-an".into(), "-frames:v".into(), "1".into()]);
                    ("png", Some(transform::visual_filter(Some(rect), None)))
                }
            }
            SourceCoordinate::VideoFrame {
                at_ms,
                frame_index,
                rect,
            } => {
                if frame_index.is_none() && !matches!(request.metadata, MediaMetadata::Image { .. })
                {
                    args.extend(["-ss".into(), transform::seconds(*at_ms).into()]);
                }
                args.extend([
                    "-i".into(),
                    input.into_os_string(),
                    "-an".into(),
                    "-frames:v".into(),
                    "1".into(),
                ]);
                let head = if let Some(index) = frame_index {
                    format!("select=eq(n\\,{index}),")
                } else if matches!(request.metadata, MediaMetadata::Image { .. }) {
                    format!("trim=start={},", transform::seconds(*at_ms))
                } else {
                    String::new()
                };
                (
                    "png",
                    Some(format!(
                        "{head}{}",
                        transform::visual_filter(rect.as_ref(), None)
                    )),
                )
            }
            SourceCoordinate::TimeSpan { start_ms, end_ms } => {
                args.extend([
                    "-ss".into(),
                    transform::seconds(*start_ms).into(),
                    "-i".into(),
                    input.into_os_string(),
                    "-t".into(),
                    transform::seconds(end_ms - start_ms).into(),
                ]);
                match request.metadata {
                    MediaMetadata::Audio { .. } => {
                        args.extend(["-vn".into(), "-c:a".into(), "pcm_s16le".into()]);
                        ("wav", None)
                    }
                    MediaMetadata::Video { .. } => {
                        args.extend([
                            "-map".into(),
                            "0:v:0".into(),
                            "-map".into(),
                            "0:a?".into(),
                            "-c:v".into(),
                            "ffv1".into(),
                            "-c:a".into(),
                            "pcm_s16le".into(),
                        ]);
                        ("mkv", None)
                    }
                    MediaMetadata::Image { frames: 2.., .. } => ("gif", None),
                    _ => return Err(Error::invalid("time span requires temporal media")),
                }
            }
            SourceCoordinate::Whole => {
                return Err(Error::invalid("whole native media does not need cropping"));
            }
            _ => return Err(Error::invalid("unsupported media crop")),
        };
        if let Some(filter) = filter {
            args.extend(["-vf".into(), filter.into()]);
        }
        let output = workspace.path().join(format!("crop.{extension}"));
        args.extend([
            "-threads".into(),
            policy.process_threads.to_string().into(),
            "-fs".into(),
            policy.max_derivative_bytes.to_string().into(),
            "-y".into(),
            output.clone().into_os_string(),
        ]);
        let result = self.ffmpeg(ctx, workspace, args, policy, cancel).await?;
        if let SourceCoordinate::VideoFrame {
            at_ms,
            frame_index,
            rect,
        } = &request.coordinate
        {
            let pts = transform::actual_pts(&result.stderr)?;
            let actual = if frame_index.is_some()
                || matches!(request.metadata, MediaMetadata::Image { .. })
            {
                pts
            } else {
                at_ms.saturating_add(pts)
            };
            coordinate = SourceCoordinate::VideoFrame {
                at_ms: actual,
                frame_index: *frame_index,
                rect: rect.clone(),
            };
        }
        let mut item = self
            .publish_output(ctx, &output, coordinate, None, policy, cancel)
            .await?;
        if let SourceCoordinate::TimeSpan { start_ms, end_ms } = &request.coordinate {
            let duration = item
                .metadata
                .duration_ms()
                .ok_or_else(|| Error::invalid("cropped media duration is unavailable"))?
                .min(end_ms - start_ms);
            item.coordinate = SourceCoordinate::TimeSpan {
                start_ms: *start_ms,
                end_ms: start_ms + duration,
            };
        }
        drop(result);
        Ok(item)
    }
}
