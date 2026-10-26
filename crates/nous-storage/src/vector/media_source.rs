//! Media authority verifies exact parts and their derivation from the retrieval target.
use crate::{not_found, source::references::validate_source};
use nous_domain::{media::*, *};
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;
use std::collections::BTreeSet;
pub(super) async fn authorize(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
) -> Result<BTreeSet<MaterialId>> {
    let roots = crate::media::roots_in(conn, namespace, subject, reference).await?;
    for material in &roots {
        validate_source(
            conn,
            namespace,
            subject,
            &EvidenceSource::Material {
                material_id: material.clone(),
            },
        )
        .await?;
    }
    Ok(roots)
}
pub(super) async fn validate(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    input: &EmbeddingInputItem,
) -> Result<()> {
    let target = input.target.item().and_then(RetrievalTarget::media);
    let roots = match target {
        Some(reference) => Some(authorize(conn, namespace, subject, reference).await?),
        None => None,
    };
    for part in &input.parts {
        let source = match part {
            EmbeddingPart::Media { source, .. }
            | EmbeddingPart::Text {
                source: Some(source),
                ..
            } => source,
            EmbeddingPart::Text { source: None, .. } => {
                if target.is_some() {
                    return Err(Error::invalid(
                        "media embedding text requires its exact persisted source",
                    ));
                }
                continue;
            }
        };
        let part_roots = authorize(conn, namespace, subject, source).await?;
        if roots
            .as_ref()
            .is_some_and(|roots| !part_roots.is_subset(roots))
        {
            return Err(Error::invalid(
                "embedding part introduces unrelated original material",
            ));
        }
        let lineage = crate::media::lineage_in(conn, namespace, subject, source, 0).await?;
        if target.is_some_and(|target| !lineage.contains(target)) {
            return Err(Error::invalid(
                "embedding part is not derived from its exact target",
            ));
        }
        match part {
            EmbeddingPart::Media { blob, .. } => {
                let actual = match source {
                    MediaReference::Original { material_id } => {
                        let material = &lineage
                            .materials
                            .iter()
                            .find(|row| row.material.id == *material_id)
                            .ok_or_else(not_found)?
                            .material;
                        BlobReference {
                            content_hash: material.content_hash.clone(),
                            media_type: material.media_type.clone(),
                            byte_len: material.byte_len as u64,
                        }
                    }
                    MediaReference::Representation { reference } => {
                        let value = lineage
                            .representations
                            .iter()
                            .find(|value| value.reference() == *reference)
                            .ok_or_else(not_found)?;
                        let RepresentationPayload::Artifact { blob, .. } = &value.payload else {
                            return Err(Error::invalid(
                                "native embedding part does not name an artifact",
                            ));
                        };
                        blob.clone()
                    }
                    _ => {
                        return Err(Error::invalid(
                            "native region input must name its committed prepared artifact",
                        ));
                    }
                };
                if actual != *blob {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "embedding artifact differs from its exact source",
                    ));
                }
            }
            EmbeddingPart::Text { text, .. } => {
                if !text_matches(&lineage, source, text)? {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "embedding text differs from its exact persisted source",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn text_matches(
    lineage: &MediaSourceLineage,
    reference: &MediaReference,
    text: &str,
) -> Result<bool> {
    match reference {
        MediaReference::Original { material_id } => {
            let material = &lineage
                .materials
                .iter()
                .find(|row| row.material.id == *material_id)
                .ok_or_else(not_found)?
                .material;
            Ok(matches!(material.metadata, MediaMetadata::Text)
                && text.len() as i64 == material.byte_len
                && format!("{:x}", Sha256::digest(text.as_bytes())) == material.content_hash)
        }
        MediaReference::SourceRegion { id } => {
            let region = lineage
                .source_regions
                .iter()
                .find(|row| row.id == *id)
                .ok_or_else(not_found)?;
            if !matches!(region.coordinate, SourceCoordinate::Whole) {
                return Err(Error::invalid(
                    "text region needs its committed mechanical text representation",
                ));
            }
            text_matches(
                lineage,
                &MediaReference::Original {
                    material_id: region.material_id.clone(),
                },
                text,
            )
        }
        MediaReference::Representation { reference } => {
            let value = lineage
                .representations
                .iter()
                .find(|row| row.reference() == *reference)
                .ok_or_else(not_found)?;
            payload_matches(&value.payload, None, text)
        }
        MediaReference::DerivedRegion { id } => {
            let region = lineage
                .derived_regions
                .iter()
                .find(|row| row.id == *id)
                .ok_or_else(not_found)?;
            let value = lineage
                .representations
                .iter()
                .find(|row| row.reference() == region.representation)
                .ok_or_else(not_found)?;
            payload_matches(&value.payload, Some(&region.selector), text)
        }
    }
}
fn payload_matches(
    payload: &RepresentationPayload,
    selector: Option<&DerivedSelector>,
    text: &str,
) -> Result<bool> {
    if let Some(expected) = representation_text(payload, selector)? {
        return Ok(expected == text);
    }
    if let RepresentationPayload::Artifact {
        blob,
        metadata: MediaMetadata::Text,
    } = payload
        && matches!(
            selector,
            None | Some(DerivedSelector::Artifact {
                coordinate: SourceCoordinate::Whole
            })
        )
    {
        return Ok(text.len() as u64 == blob.byte_len
            && format!("{:x}", Sha256::digest(text.as_bytes())) == blob.content_hash);
    }
    Err(Error::invalid(
        "text embedding input needs its committed mechanical text representation",
    ))
}
