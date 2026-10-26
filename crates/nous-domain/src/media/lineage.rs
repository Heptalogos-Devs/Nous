use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOriginal {
    pub material: Material,
    pub details: crate::MaterialDetails,
    pub details_redacted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSourceLineage {
    pub source: MediaReference,
    pub materials: Vec<MediaOriginal>,
    pub source_regions: Vec<SourceRegion>,
    pub representations: Vec<DerivedRepresentation>,
    pub derived_regions: Vec<DerivedRegion>,
    pub watermark: i64,
}
impl MediaSourceLineage {
    pub fn commit_seq(&self) -> Result<i64> {
        match &self.source {
            MediaReference::Original { material_id } => self
                .materials
                .iter()
                .find(|row| row.material.id == *material_id)
                .map(|row| row.material.commit_seq),
            MediaReference::SourceRegion { id } => self
                .source_regions
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.commit_seq),
            MediaReference::Representation { reference } => self
                .representations
                .iter()
                .find(|row| row.reference() == *reference)
                .map(|row| row.commit_seq),
            MediaReference::DerivedRegion { id } => self
                .derived_regions
                .iter()
                .find(|row| row.id == *id)
                .map(|row| row.commit_seq),
        }
        .ok_or_else(|| {
            Error::new(
                crate::ErrorCode::NotFound,
                "selected media source is unavailable",
            )
        })
    }
    pub fn source_references(&self) -> Result<Vec<MediaReference>> {
        match &self.source {
            MediaReference::Original { .. } => Some(vec![]),
            MediaReference::SourceRegion { id } => self
                .source_regions
                .iter()
                .find(|row| row.id == *id)
                .map(SourceRegion::source_references),
            MediaReference::Representation { reference } => self
                .representations
                .iter()
                .find(|row| row.reference() == *reference)
                .map(DerivedRepresentation::source_references),
            MediaReference::DerivedRegion { id } => self
                .derived_regions
                .iter()
                .find(|row| row.id == *id)
                .map(DerivedRegion::source_references),
        }
        .ok_or_else(|| {
            Error::new(
                crate::ErrorCode::NotFound,
                "selected media source is unavailable",
            )
        })
    }
    pub fn contains(&self, reference: &MediaReference) -> bool {
        match reference {
            MediaReference::Original { material_id } => self
                .materials
                .iter()
                .any(|row| row.material.id == *material_id),
            MediaReference::SourceRegion { id } => {
                self.source_regions.iter().any(|row| row.id == *id)
            }
            MediaReference::Representation { reference } => self
                .representations
                .iter()
                .any(|row| row.reference() == *reference),
            MediaReference::DerivedRegion { id } => {
                self.derived_regions.iter().any(|row| row.id == *id)
            }
        }
    }
}
