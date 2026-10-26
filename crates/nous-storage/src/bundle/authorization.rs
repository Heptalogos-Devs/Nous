//! Imported payloads stay byte-faithful; only their source qualifications are mapped.
use crate::{ReadView, storage_error};
use nous_domain::{media::*, *};
use sqlx::Row;

impl ReadView {
    pub async fn imported_model_sources(
        &mut self,
        call: &ModelCall,
        sources: &[SourceReadRef],
    ) -> Result<Vec<SourceReadRef>> {
        let ModelCallOrigin::Imported {
            installation_id,
            original_call_id,
            original_namespace_id,
            original_subject_id,
        } = &call.origin
        else {
            return Ok(sources.to_vec());
        };
        let installation = self.bundle_installation(installation_id).await?;
        if installation.namespace_id != call.namespace_id
            || installation.subject_id != call.subject_id
        {
            return Err(denied());
        }
        let resources=sqlx::query("SELECT provenance FROM bundle_resources WHERE installation_id=? ORDER BY kind,original_id,revision").bind(installation_id).fetch_all(&mut *self.tx).await.map_err(storage_error)?.into_iter().map(|row|serde_json::from_str(row.get("provenance")).map_err(Error::from)).collect::<Result<Vec<BundleResource>>>()?;
        let mut layer = BundleImportProvenance {
            installation,
            resources,
            vector_dispositions: vec![],
        };
        let ancestry = self.bundle_ancestry(installation_id).await?;
        let mut call_id = call.id.to_string();
        let mut chain = Vec::new();
        loop {
            if chain.len() >= 64 {
                return Err(denied());
            }
            let previous = layer
                .resources
                .iter()
                .find(|r| r.kind == "model_call" && r.local_id == call_id)
                .map(|r| r.original_id.clone())
                .ok_or_else(denied)?;
            let at_origin = previous == original_call_id.as_str()
                && layer.installation.source_namespace_id == *original_namespace_id
                && layer.installation.source_subject_id == *original_subject_id;
            let source_ns = layer.installation.source_namespace_id.clone();
            let source_subject = layer.installation.source_subject_id.clone();
            chain.push(layer);
            if at_origin {
                break;
            }
            call_id = previous;
            layer = ancestry
                .iter()
                .find(|candidate| {
                    candidate.installation.namespace_id == source_ns
                        && candidate.installation.subject_id == source_subject
                        && candidate
                            .resources
                            .iter()
                            .any(|r| r.kind == "model_call" && r.local_id == call_id)
                })
                .cloned()
                .ok_or_else(denied)?;
        }
        let mut mapped = sources.to_vec();
        for layer in chain.iter().rev() {
            mapped = mapped
                .iter()
                .map(|source| map_source(layer, source))
                .collect::<Result<_>>()?;
        }
        // This owner returns a qualification plan, never altered request or response bytes.
        Ok(mapped)
    }
}
fn map_id(
    layer: &BundleImportProvenance,
    kind: &str,
    id: &str,
    revision: Option<i64>,
) -> Result<String> {
    layer
        .resources
        .iter()
        .find(|r| r.kind == kind && r.original_id == id && r.revision == revision)
        .map(|r| r.local_id.clone())
        .ok_or_else(denied)
}
fn reference(layer: &BundleImportProvenance, value: &CognitiveRef) -> Result<CognitiveRef> {
    Ok(CognitiveRef {
        kind: value.kind,
        id: map_id(
            layer,
            if value.kind == CognitiveKind::Memory {
                "memory"
            } else {
                "cognition"
            },
            &value.id,
            Some(value.revision),
        )?,
        revision: value.revision,
    })
}
fn media(layer: &BundleImportProvenance, value: &MediaReference) -> Result<MediaReference> {
    Ok(match value {
        MediaReference::Original { material_id } => MediaReference::Original {
            material_id: MaterialId(map_id(layer, "material", material_id.as_str(), None)?),
        },
        MediaReference::SourceRegion { id } => MediaReference::SourceRegion {
            id: SourceRegionId(map_id(layer, "source_region", id.as_str(), None)?),
        },
        MediaReference::DerivedRegion { id } => MediaReference::DerivedRegion {
            id: DerivedRegionId(map_id(layer, "derived_region", id.as_str(), None)?),
        },
        MediaReference::Representation { reference } => MediaReference::Representation {
            reference: RepresentationRef {
                id: DerivedRepresentationId(map_id(
                    layer,
                    "representation",
                    reference.id.as_str(),
                    Some(reference.revision),
                )?),
                revision: reference.revision,
            },
        },
    })
}
fn map_source(layer: &BundleImportProvenance, value: &SourceReadRef) -> Result<SourceReadRef> {
    Ok(match value {
        SourceReadRef::Cognition { reference: value } => SourceReadRef::Cognition {
            reference: reference(layer, value)?,
        },
        SourceReadRef::Media {
            reference,
            observation_id,
        } => SourceReadRef::Media {
            reference: media(layer, reference)?,
            observation_id: observation_id
                .as_ref()
                .map(|id| map_id(layer, "observation", id.as_str(), None).map(ObservationId))
                .transpose()?,
        },
        SourceReadRef::Observation { id } => SourceReadRef::Observation {
            id: ObservationId(map_id(layer, "observation", id.as_str(), None)?),
        },
        SourceReadRef::WorkContext { .. } => return Err(denied()),
    })
}
fn denied() -> Error {
    Error::new(
        ErrorCode::Forbidden,
        "imported model payload has unavailable or unmapped original sources",
    )
}
