//! Portable subject knowledge and the authority of an installed package.
use crate::{media::*, *};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
mod descriptor;
pub use descriptor::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectPackage {
    #[serde(default)]
    pub addresses: Vec<crate::references::ReferenceBinding>,
    pub package: PackageDescriptor,
    pub content_level: BundleContentLevel,
    pub created_at: String,
    pub watermark: i64,
    pub subject: Subject,
    pub materials: Vec<MediaOriginal>,
    pub observations: Vec<Observation>,
    pub exposures: Vec<MediaExposure>,
    pub memories: Vec<Memory>,
    pub cognition: Vec<StructuredObject>,
    pub source_regions: Vec<SourceRegion>,
    pub representations: Vec<DerivedRepresentation>,
    pub derived_regions: Vec<DerivedRegion>,
    pub calls: Vec<ModelCall>,
    pub purges: Vec<PurgedMemory>,
    pub tombstones: Vec<SourceTombstone>,
    pub imports: Vec<BundleImportProvenance>,
    pub vectors: BundleVectors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundleInstallMode {
    Merge,
    Restore,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportBundleRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub package: PackageDescriptor,
    pub content_level: BundleContentLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstallBundleRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub material_id: MaterialId,
    pub mode: BundleInstallMode,
    pub placement: BundlePlacement,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleInstallation {
    pub id: String,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub source_namespace_id: NamespaceId,
    pub source_subject_id: SubjectId,
    pub package: PackageDescriptor,
    pub content_level: BundleContentLevel,
    pub placement: BundlePlacement,
    pub content_digest: String,
    pub archive_material_id: MaterialId,
    pub installed_at: String,
    pub uninstalled_at: Option<String>,
    pub commit_seq: i64,
    pub resource_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleResource {
    pub installation_id: String,
    pub kind: String,
    pub original_id: String,
    pub local_id: String,
    pub revision: Option<i64>,
    pub original_recorded_at: String,
    pub original_commit_seq: i64,
    pub original_observation: Option<Observation>,
    pub original_exposures: Vec<MediaExposure>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleImportProvenance {
    pub installation: BundleInstallation,
    pub resources: Vec<BundleResource>,
    pub vector_dispositions: Vec<BundleVectorDisposition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleVectorDisposition {
    pub space_id: String,
    pub source_hash: String,
    pub source_target: EmbeddingTarget,
    pub mapped_target: EmbeddingTarget,
    pub model_call_id: ModelCallId,
    pub status: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BundleVectors {
    pub spaces: Vec<EmbeddingSpace>,
    pub contents: Vec<EmbeddingContent>,
    pub associations: Vec<BundleEmbeddingAssociation>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingAcceptance {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub context: CallContext,
    pub time: OperationTime,
    pub commit_seq: i64,
    pub parent: Option<Box<EmbeddingAcceptance>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleEmbeddingAssociation {
    pub record: EmbeddingRecord,
    pub input: EmbeddingInputItem,
    pub acceptance: EmbeddingAcceptance,
    pub retired: bool,
}

impl SubjectPackage {
    pub fn validate(&self) -> Result<()> {
        self.validate_addresses()?;
        self.package.validate()?;
        self.subject.id.validate()?;
        self.subject.namespace_id.validate()?;
        if self.watermark <= 0 {
            return Err(Error::invalid("package watermark must be positive"));
        }
        let (ns, sub) = (&self.subject.namespace_id, &self.subject.id);
        if self
            .materials
            .iter()
            .any(|v| &v.material.namespace_id != ns)
            || self
                .observations
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .exposures
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .memories
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .cognition
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self.source_regions.iter().any(|v| &v.namespace_id != ns)
            || self
                .representations
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .derived_regions
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .calls
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
        {
            return Err(Error::invalid(
                "package records leave their declared subject scope",
            ));
        }
        if self
            .purges
            .iter()
            .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
            || self
                .tombstones
                .iter()
                .any(|v| &v.namespace_id != ns || &v.subject_id != sub)
        {
            return Err(Error::invalid(
                "package retirement facts leave their subject scope",
            ));
        }
        for object in &self.cognition {
            object.validate()?;
        }
        if self.materials.iter().any(|value| value.details_redacted) {
            return Err(Error::invalid(
                "subject package cannot omit admitted source metadata",
            ));
        }
        for memory in &self.memories {
            memory.times.validate()?;
            memory.source.validate()?;
            if let Some(id) = &memory.model_call_id
                && !self
                    .calls
                    .iter()
                    .any(|call| &call.id == id && call.state == ModelCallState::Succeeded)
            {
                return Err(Error::invalid(
                    "model Memory has no successful actual producer history in the package",
                ));
            }
        }
        for value in &self.vectors.contents {
            if value.namespace_id != *ns
                || value.subject_id != *sub
                || !self.calls.iter().any(|call| {
                    call.id == value.model_call_id && call.state == ModelCallState::Succeeded
                })
            {
                return Err(Error::invalid(
                    "packaged vector has no scoped successful actual call",
                ));
            }
        }
        for value in &self.vectors.associations {
            if value.record.namespace_id != *ns
                || value.record.subject_id != *sub
                || value.input.target != value.record.target
                || value.input.source_hash() != value.record.source_hash
            {
                return Err(Error::invalid(
                    "packaged vector association differs from its scoped parts",
                ));
            }
        }
        for tombstone in &self.tombstones {
            let RetrievalTarget::Cognition(reference) = &tombstone.target else {
                return Err(Error::invalid("package tombstone does not name cognition"));
            };
            if reference.kind != CognitiveKind::Memory
                || !self
                    .purges
                    .iter()
                    .any(|v| v.id.as_str() == reference.id && v.revision > reference.revision)
                || self.memories.iter().any(|v| v.id.as_str() == reference.id)
            {
                return Err(Error::invalid(
                    "package tombstone has no matching permanent purge fact",
                ));
            }
        }
        Ok(())
    }

    pub fn objects(&self) -> Result<Vec<BlobReference>> {
        let mut references = BTreeMap::new();
        for value in &self.materials {
            let material = &value.material;
            let blob = BlobReference {
                content_hash: material.content_hash.clone(),
                media_type: material.media_type.clone(),
                byte_len: u64::try_from(material.byte_len)
                    .map_err(|_| Error::invalid("negative material length"))?,
            };
            add_blob(&mut references, blob)?;
        }
        for value in &self.representations {
            if let RepresentationPayload::Artifact { blob, .. } = &value.payload {
                add_blob(&mut references, blob.clone())?;
            }
        }
        for value in &self.vectors.associations {
            for part in &value.input.parts {
                if let EmbeddingPart::Media { blob, .. } = part {
                    add_blob(&mut references, blob.clone())?;
                }
            }
        }
        Ok(references.into_values().collect())
    }
}
fn add_blob(values: &mut BTreeMap<String, BlobReference>, blob: BlobReference) -> Result<()> {
    blob.validate()?;
    if let Some(old) = values.insert(blob.content_hash.clone(), blob.clone())
        && old.byte_len != blob.byte_len
    {
        return Err(Error::invalid("package object has inconsistent lengths"));
    }
    Ok(())
}
