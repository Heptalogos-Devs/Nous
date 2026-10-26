use super::{Media, MediaPolicy, PreparationRequest, PreparedItem};
use crate::platform::{ProcessResult, ProcessSpec, ProcessWorkspace, StopReason};
use nous_domain::{
    CallContext, Error, ErrorCode, Result,
    media::{MediaMetadata, SourceCoordinate},
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

impl Media {
    pub(super) async fn stage(
        &self,
        request: &PreparationRequest,
        cancel: &CancellationToken,
    ) -> Result<(ProcessWorkspace, PathBuf)> {
        let workspace = self.platform.workspace()?;
        let input = workspace.path().join("source.media");
        self.objects
            .copy_to(
                &request.blob.content_hash,
                request.blob.byte_len as i64,
                &input,
                cancel,
            )
            .await?;
        Ok((workspace, input))
    }
    pub(super) async fn ffmpeg(
        &self,
        ctx: &CallContext,
        workspace: ProcessWorkspace,
        args: Vec<OsString>,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<ProcessResult> {
        let mut spec = ProcessSpec::new(&policy.ffmpeg_resource);
        spec.args = args;
        spec.timeout = Some(Duration::from_millis(policy.transform_timeout_ms));
        let result = self
            .platform
            .start(ctx, workspace, spec, cancel.clone())
            .await?
            .wait()
            .await?;
        if !matches!(result.reason, StopReason::Exited) || !result.exit.success {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "FFmpeg media preparation failed",
            )
            .with_context("resource", policy.ffmpeg_resource.clone())
            .with_context("exit", result.exit.description)
            .with_context(
                "stderr",
                String::from_utf8_lossy(&result.stderr[..result.stderr.len().min(4096)])
                    .into_owned(),
            ));
        }
        Ok(result)
    }
    pub(super) async fn publish_output(
        &self,
        ctx: &CallContext,
        path: &Path,
        coordinate: SourceCoordinate,
        at_ms: Option<u64>,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<PreparedItem> {
        let input = tokio::fs::File::open(path).await?;
        let object = self
            .objects
            .publish(input, policy.max_derivative_bytes, cancel)
            .await?;
        let (media_type, metadata) = self.inspect(ctx, &object, None, policy, cancel).await?;
        Ok(PreparedItem {
            blob: nous_domain::media::BlobReference {
                content_hash: object.digest.clone(),
                media_type,
                byte_len: object.byte_len as u64,
            },
            object: Some(object),
            metadata,
            coordinate,
            at_ms,
        })
    }
    pub(super) async fn ffmpeg_version(
        &self,
        ctx: &CallContext,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let mut spec = ProcessSpec::new(&policy.ffmpeg_resource);
        spec.args = vec!["-version".into()];
        spec.timeout = Some(Duration::from_millis(policy.probe_timeout_ms));
        let result = self.platform.run(ctx, spec, cancel.clone()).await?;
        let text = std::str::from_utf8(&result.stdout).map_err(|error| {
            Error::invalid("FFmpeg version response is not UTF-8").with_cause(&error)
        })?;
        let version = text
            .lines()
            .next()
            .filter(|line| line.starts_with("ffmpeg version "))
            .ok_or_else(|| Error::invalid("registered transform resource is not FFmpeg"))?;
        if !matches!(result.reason, StopReason::Exited) || !result.exit.success {
            return Err(Error::new(
                ErrorCode::DependencyUnavailable,
                "FFmpeg version check failed",
            ));
        }
        Ok(version.into())
    }
}
pub(super) fn base_args(policy:&MediaPolicy)->Vec<OsString> {
    vec!["-nostdin".into(),"-hide_banner".into(),"-protocol_whitelist".into(),"file,pipe".into(),
        "-max_alloc".into(),policy.max_decoder_allocation_bytes.to_string().into(),
        "-threads".into(),policy.process_threads.to_string().into()]
}

pub(super) fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}
pub(super) fn visual_filter(
    rect: Option<&nous_domain::media::PixelRect>,
    scale: Option<u32>,
) -> String {
    let mut filters = Vec::new();
    if let Some(rect) = rect {
        filters.push(format!(
            "crop=w={}:h={}:x={}:y={}:exact=1",
            rect.width, rect.height, rect.x, rect.y
        ));
    }
    if let Some(size) = scale {
        filters.push(format!(
            "scale={size}:{size}:force_original_aspect_ratio=decrease"
        ));
    }
    filters.push("showinfo".into());
    filters.join(",")
}
pub(super) fn actual_pts(stderr: &[u8]) -> Result<u64> {
    String::from_utf8_lossy(stderr)
        .lines()
        .find_map(|line| {
            let value = line
                .split_once("pts_time:")?
                .1
                .split_whitespace()
                .next()?
                .parse::<f64>()
                .ok()?;
            (value.is_finite() && value >= 0.0 && value <= u64::MAX as f64 / 1000.0)
                .then(|| (value * 1000.0).round() as u64)
        })
        .ok_or_else(|| Error::invalid("FFmpeg did not report a valid decoded frame timestamp"))
}
pub(super) fn span(request: &PreparationRequest) -> Result<(u64, u64)> {
    match request.coordinate {
        SourceCoordinate::TimeSpan { start_ms, end_ms } => Ok((start_ms, end_ms - start_ms)),
        _ => request
            .metadata
            .duration_ms()
            .filter(|duration| *duration > 0)
            .map(|duration| (0, duration))
            .ok_or_else(|| {
                Error::invalid("temporal preparation requires the actual source duration")
            }),
    }
}
pub(super) fn has_audio(metadata: &MediaMetadata) -> bool {
    matches!(
        metadata,
        MediaMetadata::Video {
            has_audio: true,
            ..
        } | MediaMetadata::Audio { .. }
    )
}
