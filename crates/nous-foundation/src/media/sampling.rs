// Uniform bounded sampling and showinfo timestamps adapted from Nous-previous model/video.ts.
use super::{Media, MediaPolicy, PreparationRequest, PreparedMedia, transform};
use nous_domain::{
    CallContext, Error, Result,
    media::{DerivationQuality, FrameSamplingReport, MediaMetadata, SourceCoordinate},
};
use tokio_util::sync::CancellationToken;

impl Media {
    pub(super) async fn sample(
        &self,
        ctx: &CallContext,
        request: &PreparationRequest,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<PreparedMedia> {
        let (start, source_duration) = transform::span(request)?;
        let bounded = source_duration.min(policy.max_sample_duration_ms);
        let version = self.ffmpeg_version(ctx, policy, cancel).await?;
        let (mut workspace, input) = self.stage(request, cancel).await?;
        let animated = matches!(request.metadata, MediaMetadata::Image { .. });
        let rect = match &request.coordinate {
            SourceCoordinate::ImageRect { rect } => Some(rect),
            _ => None,
        };
        let mut items = Vec::new();
        let mut requested = Vec::new();
        let mut sampled = Vec::new();
        let mut omitted = Vec::new();
        for ordinal in 0..policy.max_frames {
            let at = if policy.max_frames == 1 {
                0
            } else if animated {
                u64::from(ordinal) * bounded / u64::from(policy.max_frames)
            } else {
                u64::from(ordinal) * bounded.saturating_sub(policy.frame_end_margin_ms)
                    / u64::from(policy.max_frames - 1)
            };
            requested.push(at);
            let absolute = start
                .checked_add(at)
                .ok_or_else(|| Error::invalid("media timestamp overflow"))?;
            let mut args = transform::base_args(policy);
            if !animated {
                args.extend(["-ss".into(), transform::seconds(absolute).into()]);
            }
            args.extend([
                "-i".into(),
                input.clone().into_os_string(),
                "-map".into(),
                "0:v:0".into(),
                "-an".into(),
                "-frames:v".into(),
                "1".into(),
            ]);
            let filter = format!(
                "{}{}",
                if animated {
                    format!("trim=start={},", transform::seconds(absolute))
                } else {
                    String::new()
                },
                transform::visual_filter(rect, Some(policy.frame_max_dimension))
            );
            let output = workspace.path().join(format!("frame-{ordinal}.jpg"));
            args.extend([
                "-threads".into(),
                policy.process_threads.to_string().into(),
                "-vf".into(),
                filter.into(),
                "-pix_fmt".into(),
                "yuvj420p".into(),
                "-q:v".into(),
                policy.jpeg_quality.to_string().into(),
                "-fs".into(),
                policy.max_derivative_bytes.to_string().into(),
                "-y".into(),
                output.clone().into_os_string(),
            ]);
            let result = self.ffmpeg(ctx, workspace, args, policy, cancel).await?;
            let exists = tokio::fs::try_exists(&output).await?;
            if !exists {
                omitted.push(at);
                workspace = result.workspace;
                continue;
            }
            let pts = transform::actual_pts(&result.stderr)?;
            let actual = if animated {
                pts.saturating_sub(start)
            } else {
                at.saturating_add(pts)
            };
            if actual >= bounded || sampled.last().is_some_and(|previous| actual <= *previous) {
                omitted.push(at);
                workspace = result.workspace;
                continue;
            }
            let item = self
                .publish_output(
                    ctx,
                    &output,
                    SourceCoordinate::VideoFrame {
                        at_ms: start + actual,
                        frame_index: None,
                        rect: rect.cloned(),
                    },
                    Some(actual),
                    policy,
                    cancel,
                )
                .await?;
            sampled.push(actual);
            items.push(item);
            workspace = result.workspace;
        }
        if sampled.is_empty() {
            return Err(Error::invalid(
                "source produced no frames within the selected span",
            ));
        }
        let mut audio_included = false;
        let mut audio_omitted = transform::has_audio(&request.metadata);
        if audio_omitted
            && policy.include_audio
            && matches!(
                request.coordinate,
                SourceCoordinate::Whole | SourceCoordinate::TimeSpan { .. }
            )
        {
            let path = workspace.path().join("audio.wav");
            let mut args = transform::base_args(policy);
            args.extend([
                "-ss".into(),
                transform::seconds(start).into(),
                "-i".into(),
                input.into_os_string(),
                "-map".into(),
                format!("0:a:{}", policy.audio_stream_index).into(),
                "-vn".into(),
                "-t".into(),
                transform::seconds(bounded).into(),
                "-ac".into(),
                policy.audio_channels.to_string().into(),
                "-ar".into(),
                policy.audio_sample_rate.to_string().into(),
                "-c:a".into(),
                "pcm_s16le".into(),
                "-fs".into(),
                policy.max_derivative_bytes.to_string().into(),
                "-y".into(),
                path.clone().into_os_string(),
            ]);
            match self.ffmpeg(ctx, workspace, args, policy, cancel).await {
                Ok(result) => {
                    match self
                        .publish_output(
                            ctx,
                            &path,
                            SourceCoordinate::TimeSpan {
                                start_ms: start,
                                end_ms: start + bounded,
                            },
                            None,
                            policy,
                            cancel,
                        )
                        .await
                    {
                        Ok(mut item) => {
                            if let Some(actual) = item.metadata.duration_ms()
                                && actual >= bounded
                            {
                                let actual = actual.min(bounded);
                                item.coordinate = SourceCoordinate::TimeSpan {
                                    start_ms: start,
                                    end_ms: start + actual,
                                };
                                items.push(item);
                                audio_included = true;
                                audio_omitted = false;
                            }
                        }
                        Err(error) => {
                            crate::execution::check_cancelled(cancel)?;
                            tracing::warn!(error=?error,"sampled audio could not be published");
                        }
                    }
                    drop(result);
                }
                Err(error) => {
                    crate::execution::check_cancelled(cancel)?;
                    tracing::warn!(error=?error,"sampled audio was omitted");
                }
            }
        }
        let quality = DerivationQuality {
            partial: true,
            source_duration_ms: Some(source_duration),
            sampling: Some(FrameSamplingReport {
                implementation: "ffmpeg".into(),
                implementation_version: version.clone(),
                policy: format!("uniform-bounded;audio-stream={}", policy.audio_stream_index),
                sampled_duration_ms: bounded,
                requested_timestamps_ms: requested,
                sampled_timestamps_ms: sampled,
                omitted_timestamps_ms: omitted,
                audio_included,
                audio_omitted,
            }),
        };
        quality.validate(Some(source_duration))?;
        let preprocessing_digest = super::prepare::digest(request, policy, &version, &items)?;
        Ok(PreparedMedia {
            items,
            quality,
            preprocessing_digest,
        })
    }
}
