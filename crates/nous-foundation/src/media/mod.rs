//! Mechanical media admission and preparation share Content and Platform owners.
mod capture;
mod crop;
mod document;
mod policy;
mod prepare;
mod probe;
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
use nous_domain::{ConfigScope, Result};
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
