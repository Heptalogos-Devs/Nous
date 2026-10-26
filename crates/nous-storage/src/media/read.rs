use crate::{not_found, storage_error};
use nous_domain::{media::*, *};
use serde::de::DeserializeOwned;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};
pub(super) fn decode<T: DeserializeOwned>(row: SqliteRow) -> Result<T> {
    serde_json::from_str(row.get("body")).map_err(Error::from)
}
pub(super) async fn source_region(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &SourceRegionId,
) -> Result<SourceRegion> {
    sqlx::query("SELECT body FROM source_regions WHERE namespace_id=? AND id=?")
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(decode)
        .transpose()?
        .ok_or_else(not_found)
}
pub(super) async fn representation(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &RepresentationRef,
) -> Result<DerivedRepresentation> {
    MediaReference::Representation {
        reference: reference.clone(),
    }
    .validate()?;
    sqlx::query("SELECT body FROM media_representations WHERE namespace_id=? AND subject_id=? AND id=? AND revision=?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(reference.id.as_str()).bind(reference.revision)
        .fetch_optional(conn).await.map_err(storage_error)?.map(decode).transpose()?.ok_or_else(not_found)
}
pub(super) async fn derived_region(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    id: &DerivedRegionId,
) -> Result<DerivedRegion> {
    sqlx::query("SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? AND id=?")
        .bind(namespace.as_str())
        .bind(subject.as_str())
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(decode)
        .transpose()?
        .ok_or_else(not_found)
}
pub(super) async fn head(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    id: &DerivedRepresentationId,
) -> Result<Option<DerivedRepresentation>> {
    sqlx::query("SELECT body FROM media_representations WHERE namespace_id=? AND subject_id=? AND id=? ORDER BY revision DESC LIMIT 1")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(id.as_str()).fetch_optional(conn).await.map_err(storage_error)?.map(decode).transpose()
}
pub(super) async fn history(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    id: &DerivedRepresentationId,
    limit: usize,
) -> Result<Vec<DerivedRepresentation>> {
    if !(1..=1000).contains(&limit) {
        return Err(Error::invalid(
            "representation history limit must be 1..1000",
        ));
    }
    sqlx::query("SELECT body FROM media_representations WHERE namespace_id=? AND subject_id=? AND id=? ORDER BY revision DESC LIMIT ?")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(id.as_str()).bind(limit as i64).fetch_all(conn).await.map_err(storage_error)?.into_iter().map(decode).collect()
}
pub(super) async fn regions(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    reference: &RepresentationRef,
) -> Result<Vec<DerivedRegion>> {
    sqlx::query("SELECT body FROM derived_regions WHERE namespace_id=? AND subject_id=? AND representation_id=? AND representation_revision=? ORDER BY commit_seq,id")
        .bind(namespace.as_str()).bind(subject.as_str()).bind(reference.id.as_str()).bind(reference.revision)
        .fetch_all(conn).await.map_err(storage_error)?.into_iter().map(decode).collect()
}
