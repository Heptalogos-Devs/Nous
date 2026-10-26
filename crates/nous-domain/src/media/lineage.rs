use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaOriginal {
    pub material: Material,
    pub details: crate::MaterialDetails,
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
