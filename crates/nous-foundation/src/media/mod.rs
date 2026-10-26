//! Mechanical media admission and preparation share Content and Platform owners.
mod capture;
mod crop;
mod document;
mod plan;
mod policy;
pub use plan::PreparationPlan;
mod prepare;
mod probe;
mod readiness;
mod sampling;
mod text;
mod transform;
use crate::{
    configuration::Configuration,
    content::{ObjectStore, PublishedObject},
    platform::Platform,
};
pub use nous_domain::media::MediaDelivery;
use nous_domain::media::MediaMetadata;
use nous_domain::media::{BlobReference, DerivationQuality, SourceCoordinate};
use nous_domain::{ConfigScope, Error, Result};
pub use policy::{MEDIA, MediaPolicy, register_configuration};

#[derive(Clone)]
pub struct Media {
    pub(super) objects: ObjectStore,
    pub(super) configuration: Configuration,
    pub(super) platform: Platform,
}
pub struct CapturedMedia {
    pub object: PublishedObject,
    pub media_type: String,
    pub metadata: MediaMetadata,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct PreparationRequest {
    pub blob: BlobReference,
    pub metadata: MediaMetadata,
    pub coordinate: SourceCoordinate,
    pub delivery: MediaDelivery,
}
impl PreparationRequest {
    pub fn validate(&self) -> Result<()> {
        self.blob.validate()?;
        self.metadata.validate(&self.blob.media_type)?;
        self.coordinate
            .validate_bounds(self.blob.byte_len, &self.metadata)?;
        if matches!(self.delivery, MediaDelivery::SampledFrames) {
            if !matches!(
                self.coordinate,
                SourceCoordinate::Whole
                    | SourceCoordinate::TimeSpan { .. }
                    | SourceCoordinate::ImageRect { .. }
            ) || !matches!(
                self.metadata,
                MediaMetadata::Video { .. } | MediaMetadata::Image { frames: 2.., .. }
            ) {
                return Err(Error::invalid(
                    "frame sampling requires a whole, temporal or spatial video or animation locator",
                ));
            }
            if self
                .coordinate
                .duration_ms(&self.metadata)
                .is_none_or(|duration| duration == 0)
            {
                return Err(Error::invalid(
                    "frame sampling requires the actual selected source duration",
                ));
            }
        }
        Ok(())
    }
}
pub struct PreparedItem {
    pub blob: BlobReference,
    pub object: Option<PublishedObject>,
    pub metadata: MediaMetadata,
    pub coordinate: SourceCoordinate,
    pub at_ms: Option<u64>,
}
pub struct PreparedMedia {
    pub items: Vec<PreparedItem>,
    pub quality: DerivationQuality,
    pub preprocessing_digest: String,
}
impl Media {
    pub fn new(objects: ObjectStore, configuration: Configuration, platform: Platform) -> Self {
        Self {
            objects,
            configuration,
            platform,
        }
    }
    pub(super) async fn policy(&self) -> Result<MediaPolicy> {
        self.configuration
            .snapshot(&ConfigScope::Instance, false)
            .await?
            .get(MEDIA)
    }
}
