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
    Ok(load(conn, namespace, subject, reference)
        .await?
        .materials
        .into_iter()
        .map(|material| material.id)
        .collect())
}

struct LoadedGraph {
    materials: Vec<Material>,
    source_regions: Vec<SourceRegion>,
    representations: Vec<DerivedRepresentation>,
    derived_regions: Vec<DerivedRegion>,
}

async fn load(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
) -> Result<LoadedGraph> {
    reference.validate()?;
    let mut pending = vec![reference.clone()];
    let mut visited = BTreeSet::new();
    let mut graph = LoadedGraph {
        materials: Vec::new(),
        source_regions: Vec::new(),
        representations: Vec::new(),
        derived_regions: Vec::new(),
    };
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
                let material = records::material(conn, namespace, &material_id)
                    .await?
                    .ok_or_else(crate::not_found)?;
                graph.materials.push(material);
            }
            MediaReference::SourceRegion { id } => {
                let value = read::source_region(conn, namespace, &id).await?;
                pending.extend(value.source_references());
                graph.source_regions.push(value);
            }
            MediaReference::Representation { reference } => {
                let value = read::representation(conn, namespace, subject, &reference).await?;
                pending.extend(value.source_references());
                graph.representations.push(value);
            }
            MediaReference::DerivedRegion { id } => {
                let value = read::derived_region(conn, namespace, subject, &id).await?;
                pending.extend(value.source_references());
                graph.derived_regions.push(value);
            }
        }
    }
    if graph.materials.is_empty() {
        return Err(Error::invalid("media reference has no original source"));
    }
    Ok(graph)
}

pub(super) async fn lineage(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &MediaReference,
    watermark: i64,
) -> Result<MediaSourceLineage> {
    let graph = load(conn, namespace, subject, reference).await?;
    let mut materials = Vec::new();
    for material in graph.materials {
        let details =
            crate::source::materials::material_details(conn, namespace, &material.id).await?;
        materials.push(MediaOriginal {
            material,
            details,
            details_redacted: false,
        });
    }
    Ok(MediaSourceLineage {
        source: reference.clone(),
        materials,
        source_regions: graph.source_regions,
        representations: graph.representations,
        derived_regions: graph.derived_regions,
        watermark,
    })
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
            Ok(region.coordinate.duration_ms(&metadata))
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
                ) => coordinate.duration_ms(metadata),
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
