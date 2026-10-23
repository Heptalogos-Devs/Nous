//! Typed original/derived media locators and containment rules.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}
impl PixelRect {
    fn validate(&self, width: u32, height: u32) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none_or(|end| end > width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|end| end > height)
        {
            return Err(Error::invalid("pixel region is outside the source image"));
        }
        Ok(())
    }
    fn contains(&self, child: &Self) -> bool {
        child.x >= self.x
            && child.y >= self.y
            && child
                .x
                .checked_add(child.width)
                .zip(self.x.checked_add(self.width))
                .is_some_and(|(a, b)| a <= b)
            && child
                .y
                .checked_add(child.height)
                .zip(self.y.checked_add(self.height))
                .is_some_and(|(a, b)| a <= b)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceCoordinate {
    Whole,
    ByteRange {
        start: u64,
        end: u64,
    },
    TextSpan {
        start: u64,
        end: u64,
    },
    ImageRect {
        rect: PixelRect,
    },
    TimeSpan {
        start_ms: u64,
        end_ms: u64,
    },
    VideoFrame {
        at_ms: u64,
        frame_index: Option<u64>,
        rect: Option<PixelRect>,
    },
    Page {
        number: u32,
    },
}
impl SourceCoordinate {
    pub fn validate_bounds(&self, byte_len: u64, metadata: &MediaMetadata) -> Result<()> {
        let valid = match self {
            Self::Whole => true,
            Self::ByteRange { start, end } => start < end && *end <= byte_len,
            Self::TextSpan { start, end } => {
                matches!(metadata, MediaMetadata::Text) && start < end && *end <= byte_len
            }
            Self::ImageRect { rect } => {
                let (w, h) = metadata.dimensions().ok_or_else(|| {
                    Error::invalid("pixel region requires decoded image dimensions")
                })?;
                rect.validate(w, h)?;
                true
            }
            Self::TimeSpan { start_ms, end_ms } => {
                matches!(
                    metadata,
                    MediaMetadata::Audio { .. }
                        | MediaMetadata::Video { .. }
                        | MediaMetadata::Image { frames: 2.., .. }
                ) && start_ms < end_ms
                    && metadata
                        .duration_ms()
                        .is_some_and(|duration| *end_ms <= duration)
            }
            Self::VideoFrame {
                at_ms,
                frame_index,
                rect,
            } => {
                let temporal = matches!(
                    metadata,
                    MediaMetadata::Video { .. } | MediaMetadata::Image { frames: 2.., .. }
                ) && metadata
                    .duration_ms()
                    .is_some_and(|duration| *at_ms <= duration);
                let frame_valid = match metadata {
                    MediaMetadata::Image { frames, .. } => {
                        frame_index.is_none_or(|index| index < u64::from(*frames))
                    }
                    _ => true,
                };
                if let Some(rect) = rect {
                    let (w, h) = metadata
                        .dimensions()
                        .ok_or_else(|| Error::invalid("frame crop requires decoded dimensions"))?;
                    rect.validate(w, h)?;
                }
                temporal && frame_valid
            }
            Self::Page { number } => {
                matches!(metadata,MediaMetadata::Document{pages:Some(pages)}if *number>=1&&number<=pages)
            }
        };
        if !valid {
            return Err(Error::invalid(
                "source coordinate is outside its original media",
            ));
        }
        Ok(())
    }
    pub fn contains(&self, child: &Self) -> bool {
        match (self, child) {
            (Self::Whole, _) => true,
            (Self::ByteRange { start: a, end: b }, Self::ByteRange { start: c, end: d })
            | (Self::TextSpan { start: a, end: b }, Self::TextSpan { start: c, end: d }) => {
                a <= c && d <= b
            }
            (Self::ImageRect { rect: a }, Self::ImageRect { rect: b }) => a.contains(b),
            (
                Self::TimeSpan {
                    start_ms: a,
                    end_ms: b,
                },
                Self::TimeSpan {
                    start_ms: c,
                    end_ms: d,
                },
            ) => a <= c && d <= b,
            (Self::TimeSpan { start_ms, end_ms }, Self::VideoFrame { at_ms, .. }) => {
                start_ms <= at_ms && at_ms <= end_ms
            }
            (
                Self::VideoFrame {
                    at_ms: a,
                    frame_index: i,
                    rect: None,
                },
                Self::VideoFrame {
                    at_ms: b,
                    frame_index: j,
                    ..
                },
            ) => a == b && i == j,
            (
                Self::VideoFrame {
                    at_ms: a,
                    frame_index: i,
                    rect: Some(x),
                },
                Self::VideoFrame {
                    at_ms: b,
                    frame_index: j,
                    rect: Some(y),
                },
            ) => a == b && i == j && x.contains(y),
            (Self::Page { number: a }, Self::Page { number: b }) => a == b,
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRegion {
    pub id: SourceRegionId,
    pub namespace_id: NamespaceId,
    pub material_id: MaterialId,
    pub parent: Option<SourceRegionId>,
    pub coordinate: SourceCoordinate,
    pub coordinate_hash: String,
    pub recorded_at: String,
    pub commit_seq: i64,
}
impl SourceRegion {
    pub fn reference(&self) -> MediaReference {
        MediaReference::SourceRegion {
            id: self.id.clone(),
        }
    }
    pub fn source_references(&self) -> Vec<MediaReference> {
        let mut refs = vec![MediaReference::Original {
            material_id: self.material_id.clone(),
        }];
        if let Some(id) = &self.parent {
            refs.push(MediaReference::SourceRegion { id: id.clone() });
        }
        refs
    }
    pub fn coordinate_key_material(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&(
            &self.namespace_id,
            &self.material_id,
            &self.parent,
            &self.coordinate,
        ))?)
    }
    pub fn validate_against(
        &self,
        material: &Material,
        metadata: &MediaMetadata,
        parent: Option<&Self>,
    ) -> Result<()> {
        self.id.validate()?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.material_id.as_str())?;
        valid_digest(&self.coordinate_hash)?;
        bounded_text(&self.recorded_at, 128, "recorded_at", true)?;
        if self.namespace_id != material.namespace_id
            || self.material_id != material.id
            || self.commit_seq < 0
            || material.byte_len <= 0
        {
            return Err(Error::invalid(
                "source region has different original identity or scope",
            ));
        }
        metadata.validate(&material.media_type)?;
        self.coordinate
            .validate_bounds(material.byte_len as u64, metadata)?;
        match (&self.parent, parent) {
            (None, None) => {}
            (Some(id), Some(parent))
                if id == &parent.id
                    && self.id != parent.id
                    && parent.material_id == self.material_id
                    && parent.namespace_id == self.namespace_id
                    && parent.coordinate.contains(&self.coordinate) => {}
            _ => {
                return Err(Error::invalid(
                    "source region parent is missing, cyclic or does not contain the region",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InterpretationSelector {
    Summary,
    Observation { index: u32 },
    Mention { index: u32 },
    EmbeddedText { index: u32 },
    SourceText { index: u32 },
    Speech { index: u32 },
    Interpretation { index: u32 },
    Uncertainty { index: u32 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivedSelector {
    TextSpan { start: u64, end: u64 },
    Interpretation { item: InterpretationSelector },
    Artifact { coordinate: SourceCoordinate },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivedRegion {
    pub id: DerivedRegionId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub representation: RepresentationRef,
    pub parent: Option<DerivedRegionId>,
    pub selector: DerivedSelector,
    pub recorded_at: String,
    pub commit_seq: i64,
}
impl DerivedRegion {
    pub fn reference(&self) -> MediaReference {
        MediaReference::DerivedRegion {
            id: self.id.clone(),
        }
    }
    pub fn source_references(&self) -> Vec<MediaReference> {
        let mut refs = vec![MediaReference::Representation {
            reference: self.representation.clone(),
        }];
        if let Some(id) = &self.parent {
            refs.push(MediaReference::DerivedRegion { id: id.clone() });
        }
        refs
    }
    pub fn validate_against(
        &self,
        representation: &DerivedRepresentation,
        parent: Option<&Self>,
    ) -> Result<()> {
        self.id.validate()?;
        validate_uuid(self.namespace_id.as_str())?;
        validate_uuid(self.subject_id.as_str())?;
        MediaReference::Representation {
            reference: self.representation.clone(),
        }
        .validate()?;
        bounded_text(&self.recorded_at, 128, "recorded_at", true)?;
        if self.namespace_id != representation.namespace_id
            || self.subject_id != representation.subject_id
            || self.representation != representation.reference()
            || self.commit_seq < 0
        {
            return Err(Error::invalid(
                "derived region differs from its exact scoped representation",
            ));
        }
        let valid = match (&self.selector, &representation.payload) {
            (DerivedSelector::TextSpan { start, end }, RepresentationPayload::Text { text }) => {
                start < end
                    && *end <= text.len() as u64
                    && text.is_char_boundary(*start as usize)
                    && text.is_char_boundary(*end as usize)
            }
            (
                DerivedSelector::Interpretation { item },
                RepresentationPayload::Interpretation { value },
            ) => item.exists(value),
            (
                DerivedSelector::Artifact { coordinate },
                RepresentationPayload::Artifact { blob, metadata },
            ) => {
                coordinate.validate_bounds(blob.byte_len, metadata)?;
                true
            }
            _ => false,
        };
        if !valid {
            return Err(Error::invalid(
                "derived region selector is outside its payload",
            ));
        }
        match (&self.parent, parent) {
            (None, None) => {}
            (Some(id), Some(parent))
                if id == &parent.id
                    && self.id != parent.id
                    && parent.namespace_id == self.namespace_id
                    && parent.subject_id == self.subject_id
                    && parent.representation == self.representation
                    && selector_contains(&parent.selector, &self.selector) => {}
            _ => {
                return Err(Error::invalid(
                    "derived region parent is missing, cyclic or incompatible",
                ));
            }
        }
        Ok(())
    }
}
fn selector_contains(parent: &DerivedSelector, child: &DerivedSelector) -> bool {
    match (parent, child) {
        (
            DerivedSelector::TextSpan { start: a, end: b },
            DerivedSelector::TextSpan { start: c, end: d },
        ) => a <= c && d <= b,
        (
            DerivedSelector::Interpretation { item: a },
            DerivedSelector::Interpretation { item: b },
        ) => a == b,
        (
            DerivedSelector::Artifact { coordinate: a },
            DerivedSelector::Artifact { coordinate: b },
        ) => a.contains(b),
        _ => false,
    }
}
