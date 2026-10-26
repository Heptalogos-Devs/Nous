use crate::configuration::{ConfigDefinition, ConfigKey, ConfigRegistryBuilder, ConfigWritePolicy};
use nous_domain::{ConfigApplyMode, ConfigLevel, Error, Result, validate_name};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MEDIA: ConfigKey<MediaPolicy> = ConfigKey::new("media");
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MediaPolicy {
    pub max_original_bytes: u64,
    pub max_derivative_bytes: u64,
    pub max_buffered_text_bytes: u64,
    pub probe_resource: String,
    pub ffmpeg_resource: String,
    pub pdf_resource: Option<String>,
    pub probe_timeout_ms: u64,
    pub transform_timeout_ms: u64,
    pub max_frames: u32,
    pub max_sample_duration_ms: u64,
    pub frame_end_margin_ms: u64,
    pub frame_max_dimension: u32,
    pub include_audio: bool,
    pub audio_channels: u16,
    pub audio_sample_rate: u32,
    pub audio_stream_index: u16,
    pub process_threads: u16,
    pub max_decoder_allocation_bytes: u64,
    pub jpeg_quality: u8,
}
impl Default for MediaPolicy {
    fn default() -> Self {
        Self {
            max_original_bytes: 512 * 1024 * 1024,
            max_derivative_bytes: 64 * 1024 * 1024,
            max_buffered_text_bytes: 4 * 1024 * 1024,
            probe_resource: "ffprobe".into(),
            ffmpeg_resource: "ffmpeg".into(),
            pdf_resource: None,
            probe_timeout_ms: 30_000,
            transform_timeout_ms: 120_000,
            max_frames: 8,
            max_sample_duration_ms: 60_000,
            frame_end_margin_ms: 100,
            frame_max_dimension: 512,
            include_audio: true,
            audio_channels: 1,
            audio_sample_rate: 16_000,
            audio_stream_index: 0,
            process_threads: 1,
            max_decoder_allocation_bytes: 64 * 1024 * 1024,
            jpeg_quality: 3,
        }
    }
}
impl MediaPolicy {
    pub fn validate(&self) -> Result<()> {
        for bytes in [
            self.max_original_bytes,
            self.max_derivative_bytes,
            self.max_buffered_text_bytes,
        ] {
            if bytes == 0 || bytes > i64::MAX as u64 {
                return Err(Error::invalid(
                    "media byte limits must be positive authority lengths",
                ));
            }
        }
        if self.max_buffered_text_bytes > 64 * 1024 * 1024
            || !(1..=64).contains(&self.max_frames)
            || !(1..=86_400_000).contains(&self.max_sample_duration_ms)
            || !(1..=16384).contains(&self.frame_max_dimension)
            || !(1..=8).contains(&self.audio_channels)
            || !(8000..=192000).contains(&self.audio_sample_rate)
            || !(1..=256).contains(&self.process_threads)
            || !(1..=i64::MAX as u64).contains(&self.max_decoder_allocation_bytes)
            || !(1..=31).contains(&self.jpeg_quality)
            || !(1..=86_400_000).contains(&self.probe_timeout_ms)
            || !(1..=86_400_000).contains(&self.transform_timeout_ms)
        {
            return Err(Error::invalid(
                "media sampling and execution limits are out of range",
            ));
        }
        validate_name(&self.probe_resource, "probe resource")?;
        validate_name(&self.ffmpeg_resource, "FFmpeg resource")?;
        if let Some(resource) = &self.pdf_resource {
            validate_name(resource, "PDF resource")?;
        }
        Ok(())
    }
}
pub fn register_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(MEDIA,ConfigDefinition{owner:"media".into(),description:"Original and derivative stream limits and registered media preprocessing resources.".into(),
        default:MediaPolicy::default(),scopes:vec![ConfigLevel::Instance],apply_mode:ConfigApplyMode::Live,write_policy:ConfigWritePolicy::Scoped},MediaPolicy::validate)
}
