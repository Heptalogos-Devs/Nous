use super::{Media, MediaPolicy};
use crate::platform::{ProcessSpec, StopReason};
use nous_domain::{
    CallContext, Error, ErrorCode, Result,
    media::{BlobReference, MediaMetadata},
};
use serde_json::Value;
use std::{ffi::OsString, time::Duration};
use tokio_util::sync::CancellationToken;

impl Media {
    pub(super) async fn probe_blob(
        &self,
        ctx: &CallContext,
        blob: &BlobReference,
        policy: &MediaPolicy,
        cancel: &CancellationToken,
    ) -> Result<(String, MediaMetadata)> {
        let workspace = self.platform.workspace()?;
        let input = workspace.path().join("source.media");
        self.objects
            .copy_to(&blob.content_hash, blob.byte_len as i64, &input, cancel)
            .await?;
        let mut spec = ProcessSpec::new(&policy.probe_resource);
        spec.timeout = Some(Duration::from_millis(policy.probe_timeout_ms));
        spec.args = [
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        if blob.media_type.starts_with("image/") {
            spec.args.push("-count_frames".into());
        }
        spec.args.extend(["-max_alloc".into(),policy.max_decoder_allocation_bytes.to_string().into(),"-threads".into(),policy.process_threads.to_string().into()]);
        spec.args.push(input.into_os_string());
        let output = self
            .platform
            .start(ctx, workspace, spec, cancel.clone())
            .await?
            .wait()
            .await?;
        if !matches!(output.reason, StopReason::Exited) || !output.exit.success {
            return Err(
                Error::new(ErrorCode::DependencyUnavailable, "media probing failed")
                    .with_context("resource", policy.probe_resource.clone())
                    .with_context("exit", output.exit.description),
            );
        }
        let probe: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            Error::invalid("ffprobe returned invalid metadata JSON").with_cause(&error)
        })?;
        metadata(&probe, &blob.media_type)
    }
}

fn metadata(probe: &Value, mime: &str) -> Result<(String, MediaMetadata)> {
    let streams = probe
        .get("streams")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::invalid("media probe has no stream inventory"))?;
    let video = streams.iter().find(|stream| {
        stream["codec_type"] == "video"
            && stream
                .pointer("/disposition/attached_pic")
                .and_then(Value::as_u64)
                != Some(1)
    });
    let audio = streams
        .iter()
        .find(|stream| stream["codec_type"] == "audio");
    let duration_ms = seconds_ms(probe.pointer("/format/duration")).or_else(|| {
        streams
            .iter()
            .filter_map(|stream| seconds_ms(stream.get("duration")))
            .max()
    });
    if mime.starts_with("image/") {
        let stream = video.ok_or_else(|| Error::invalid("image probe has no image stream"))?;
        let width = number(stream.get("width"))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::invalid("image width is unavailable"))?;
        let height = number(stream.get("height"))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::invalid("image height is unavailable"))?;
        let frames = number(stream.get("nb_read_frames"))
            .or_else(|| number(stream.get("nb_frames")))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::invalid("image frame count is unavailable"))?;
        return Ok((
            mime.into(),
            MediaMetadata::Image {
                width,
                height,
                frames,
                duration_ms,
            },
        ));
    }
    if let Some(stream) = video {
        let width = number(stream.get("width"))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0);
        let height = number(stream.get("height"))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0);
        return Ok((
            if mime.starts_with("video/") {
                mime.into()
            } else {
                mime.replacen("audio/", "video/", 1)
            },
            MediaMetadata::Video {
                duration_ms,
                width,
                height,
                has_audio: audio.is_some(),
            },
        ));
    }
    if let Some(stream) = audio {
        let channels = number(stream.get("channels"))
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0);
        let sample_rate = number(stream.get("sample_rate"))
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0);
        let mime = if mime.starts_with("audio/") {
            mime.into()
        } else {
            mime.replacen("video/", "audio/", 1)
        };
        return Ok((
            mime,
            MediaMetadata::Audio {
                duration_ms,
                channels,
                sample_rate,
            },
        ));
    }
    Err(Error::invalid(
        "media probe found no supported media streams",
    ))
}
fn number(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}
pub(super) fn seconds_ms(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    let seconds = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    (seconds.is_finite() && seconds > 0.0 && seconds <= u64::MAX as f64 / 1000.0)
        .then(|| (seconds * 1000.0).round() as u64)
}
