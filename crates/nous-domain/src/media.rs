//! Original media locators and ordered derivation provenance.
use crate::identity::validate_uuid;
use crate::{
    Error, Material, MaterialId, ModelCallId, NamespaceId, ObservationId, Result, SubjectId,
    TimePoint,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
mod interpretation;
pub use interpretation::*;
mod lineage;
pub use lineage::*;
mod text;
pub use text::representation_text;

mod regions;
pub use regions::*;
mod derivation;
pub use derivation::*;
mod sampling;
pub use sampling::*;
mod workflow;
pub use workflow::*;
identity!(SourceRegionId);
identity!(DerivedRepresentationId);
identity!(DerivedRegionId);

/// Metadata accompanies an independent streaming body in App/object storage.
/// Hashes and lengths are expectations; authority records actual received bytes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PutMediaRequest {
    pub namespace_id: NamespaceId,
    pub media_type: String,
    pub declared_byte_len: Option<u64>,
    pub expected_content_hash: Option<String>,
    pub source: Option<String>,
    pub event_time: Option<TimePoint>,
}
impl PutMediaRequest {
    pub fn validate(&self) -> Result<()> {
        validate_uuid(self.namespace_id.as_str())?;
        canonical_media_type(&self.media_type)?;
        if self
            .declared_byte_len
            .is_some_and(|len| len == 0 || len > i64::MAX as u64)
        {
            return Err(Error::invalid(
                "declared original length is outside the storage range",
            ));
        }
        if let Some(hash) = &self.expected_content_hash {
            valid_digest(hash)?;
        }
        if let Some(source) = &self.source {
            bounded_text(source, 4096, "material source", false)?;
        }
        if let Some(time) = &self.event_time {
            time.validate()?;
        }
        Ok(())
    }
    pub fn validate_received(&self, byte_len: u64, content_hash: &str) -> Result<()> {
        self.validate()?;
        valid_digest(content_hash)?;
        if byte_len == 0
            || byte_len > i64::MAX as u64
            || self
                .declared_byte_len
                .is_some_and(|expected| expected != byte_len)
            || self
                .expected_content_hash
                .as_ref()
                .is_some_and(|expected| expected != content_hash)
        {
            return Err(Error::invalid(
                "original stream differs from declared length or digest",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaMetadata {
    Text,
    Image {
        width: u32,
        height: u32,
        frames: u32,
        duration_ms: Option<u64>,
    },
    Audio {
        duration_ms: Option<u64>,
        channels: Option<u16>,
        sample_rate: Option<u32>,
    },
    Video {
        duration_ms: Option<u64>,
        width: Option<u32>,
        height: Option<u32>,
        has_audio: bool,
    },
    Document {
        pages: Option<u32>,
    },
    Binary,
}
impl MediaMetadata {
    pub fn validate(&self, media_type: &str) -> Result<()> {
        let mime = canonical_media_type(media_type)?;
        let valid = match self {
            Self::Text => {
                mime.starts_with("text/") || mime.contains("json") || mime.contains("xml")
            }
            Self::Image {
                width,
                height,
                frames,
                ..
            } => mime.starts_with("image/") && *width > 0 && *height > 0 && *frames > 0,
            Self::Audio {
                channels,
                sample_rate,
                ..
            } => {
                mime.starts_with("audio/")
                    && channels.is_none_or(|n| n > 0)
                    && sample_rate.is_none_or(|n| n > 0)
            }
            Self::Video { width, height, .. } => {
                mime.starts_with("video/")
                    && matches!((width, height), (None, None) | (Some(1..), Some(1..)))
            }
            Self::Document { pages } => {
                !mime.starts_with("image/")
                    && !mime.starts_with("audio/")
                    && !mime.starts_with("video/")
                    && pages.is_none_or(|n| n > 0)
            }
            Self::Binary => true,
        };
        if !valid {
            return Err(Error::invalid(
                "media metadata does not match its type or dimensions",
            ));
        }
        Ok(())
    }
    pub fn duration_ms(&self) -> Option<u64> {
        match self {
            Self::Image { duration_ms, .. }
            | Self::Audio { duration_ms, .. }
            | Self::Video { duration_ms, .. } => *duration_ms,
            _ => None,
        }
    }
    fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            Self::Image { width, height, .. } => Some((*width, *height)),
            Self::Video {
                width: Some(width),
                height: Some(height),
                ..
            } => Some((*width, *height)),
            _ => None,
        }
    }
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct RepresentationRef {
    pub id: DerivedRepresentationId,
    pub revision: i64,
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaReference {
    Original { material_id: MaterialId },
    SourceRegion { id: SourceRegionId },
    Representation { reference: RepresentationRef },
    DerivedRegion { id: DerivedRegionId },
}
impl MediaReference {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Original { material_id } => validate_uuid(material_id.as_str()),
            Self::SourceRegion { id } => id.validate(),
            Self::DerivedRegion { id } => id.validate(),
            Self::Representation { reference } => {
                reference.id.validate()?;
                if reference.revision < 1 {
                    return Err(Error::invalid(
                        "representation reference requires an exact positive revision",
                    ));
                }
                Ok(())
            }
        }
    }
}

pub fn canonical_media_type(value: &str) -> Result<String> {
    if value.len() > 255 || value.chars().any(char::is_control) {
        return Err(Error::invalid("media type is invalid"));
    }
    let stem = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let valid = stem.split_once('/').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && a.bytes()
                .chain(b.bytes())
                .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&c))
    });
    if !valid {
        return Err(Error::invalid("media type must have a valid type/subtype"));
    }
    Ok(stem)
}
fn valid_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(Error::invalid("digest must be lowercase SHA-256 hex"));
    }
    Ok(())
}
fn bounded_text(value: &str, max: usize, field: &str, required: bool) -> Result<()> {
    if value.len() > max || value.contains('\0') || (required && value.trim().is_empty()) {
        return Err(Error::invalid(format!(
            "{field} is empty, too long or contains NUL"
        )));
    }
    Ok(())
}

/// A Contact's exact delivered locator. Persisting an artifact or a derivation
/// does not create this fact; App binds it to an actual existing Contact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaExposure {
    pub observation_id: ObservationId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub locator: MediaReference,
    pub access: ChannelAccess,
    pub recorded_at: String,
    pub commit_seq: i64,
}
impl MediaExposure {
    /// `resolved_roots` and `actual_access` come from the storage lineage and
    /// delivery owner. They are not fields accepted as caller assertions.
    pub fn validate_against(
        &self,
        observation: &crate::Observation,
        resolved_roots: &BTreeSet<MaterialId>,
        actual_access: ChannelAccess,
    ) -> Result<()> {
        validate_uuid(self.observation_id.as_str())?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        self.locator.validate()?;
        bounded_text(&self.recorded_at, 128, "exposure recorded_at", true)?;
        if observation.id != self.observation_id
            || observation.namespace_id != self.namespace_id
            || observation.subject_id != self.subject_id
            || observation.kind != crate::ObservationKind::Contact
            || self.commit_seq < observation.commit_seq
            || !resolved_roots.contains(&observation.material_id)
            || self.access != actual_access
        {
            return Err(Error::invalid(
                "media exposure differs from actual Contact, source lineage or delivered access",
            ));
        }
        if let MediaReference::Original { material_id } = &self.locator
            && material_id != &observation.material_id
        {
            return Err(Error::invalid(
                "original exposure must name the Contact material",
            ));
        }
        Ok(())
    }
}
