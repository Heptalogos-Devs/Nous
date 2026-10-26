use super::read;
use crate::authority::records;
use nous_domain::{media::*, *};
use sqlx::SqliteConnection;
use std::collections::BTreeSet;
pub(super) async fn roots(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
) -> Result<BTreeSet<MaterialId>> {
    reference.validate()?;
    let mut pending = vec![reference.clone()];
    let mut visited = BTreeSet::new();
    let mut roots = BTreeSet::new();
    while let Some(reference) = pending.pop() {
        if !visited.insert(reference.clone()) {
            continue;
        }
        if visited.len() > 4096 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "media source graph exceeds 4096 locators",
            ));
        }
        match reference {
            MediaReference::Original { material_id } => {
                records::material(conn, namespace, &material_id)
                    .await?
                    .ok_or_else(crate::not_found)?;
                roots.insert(material_id);
            }
            MediaReference::SourceRegion { id } => pending.extend(
                read::source_region(conn, namespace, &id)
                    .await?
                    .source_references(),
            ),
            MediaReference::Representation { reference } => pending.extend(
                read::representation(conn, namespace, subject, &reference)
                    .await?
                    .source_references(),
            ),
            MediaReference::DerivedRegion { id } => pending.extend(
                read::derived_region(conn, namespace, subject, &id)
                    .await?
                    .source_references(),
            ),
        }
    }
    if roots.is_empty() {
        return Err(Error::invalid("media reference has no original source"));
    }
    Ok(roots)
}
pub(super) async fn duration(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
) -> Result<Option<u64>> {
    match reference {
        MediaReference::Original { material_id } => {
            Ok(records::material(conn, namespace, material_id)
                .await?
                .ok_or_else(crate::not_found)?
                .metadata
                .duration_ms())
        }
        MediaReference::SourceRegion { id } => {
            let region = read::source_region(conn, namespace, id).await?;
            let metadata = records::material(conn, namespace, &region.material_id)
                .await?
                .ok_or_else(crate::not_found)?
                .metadata;
            Ok(coordinate_duration(&region.coordinate, &metadata))
        }
        MediaReference::Representation { reference } => Ok(representation_duration(
            &read::representation(conn, namespace, subject, reference).await?,
        )),
        MediaReference::DerivedRegion { id } => {
            let region = read::derived_region(conn, namespace, subject, id).await?;
            let representation =
                read::representation(conn, namespace, subject, &region.representation).await?;
            Ok(match (&region.selector, &representation.payload) {
                (
                    DerivedSelector::Artifact { coordinate },
                    RepresentationPayload::Artifact { metadata, .. },
                ) => coordinate_duration(coordinate, metadata),
                _ => representation_duration(&representation),
            })
        }
    }
}
fn representation_duration(representation: &DerivedRepresentation) -> Option<u64> {
    match &representation.payload {
        RepresentationPayload::Artifact { metadata, .. } => metadata.duration_ms(),
        _ => representation.quality.source_duration_ms,
    }
}
fn coordinate_duration(coordinate: &SourceCoordinate, metadata: &MediaMetadata) -> Option<u64> {
    match coordinate {
        SourceCoordinate::Whole | SourceCoordinate::ImageRect { .. } => metadata.duration_ms(),
        SourceCoordinate::TimeSpan { start_ms, end_ms } => Some(end_ms - start_ms),
        _ => None,
    }
}
