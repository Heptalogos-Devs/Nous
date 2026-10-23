use nous_domain::{
    Material, MaterialId, Namespace, NamespaceId, Observation, ObservationId, Result, Subject,
    SubjectId, TimePoint,
};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

use crate::storage_error;

pub(crate) async fn namespace(
    conn: &mut SqliteConnection,
    id: &NamespaceId,
) -> Result<Option<Namespace>> {
    sqlx::query("SELECT * FROM namespaces WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(namespace_row)
        .transpose()
}

pub(crate) async fn subject(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &SubjectId,
) -> Result<Option<Subject>> {
    sqlx::query("SELECT * FROM subjects WHERE namespace_id=? AND id=?")
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(subject_row)
        .transpose()
}

pub(crate) async fn material(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MaterialId,
) -> Result<Option<Material>> {
    sqlx::query("SELECT * FROM materials WHERE namespace_id=? AND id=?")
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(material_row)
        .transpose()
}

pub(crate) async fn observation(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &ObservationId,
) -> Result<Option<Observation>> {
    sqlx::query("SELECT * FROM observations WHERE namespace_id=? AND id=?")
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(observation_row)
        .transpose()
}

pub(crate) fn namespace_row(row: SqliteRow) -> Result<Namespace> {
    Ok(Namespace {
        id: NamespaceId(row.get("id")),
        name: row.get("name"),
        revision: row.get("revision"),
        commit_seq: row.get("commit_seq"),
    })
}

pub(crate) fn subject_row(row: SqliteRow) -> Result<Subject> {
    Ok(Subject {
        id: SubjectId(row.get("id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        name: row.get("name"),
        capabilities: serde_json::from_str(row.get("capabilities")).map_err(storage_error)?,
        revision: row.get("revision"),
        commit_seq: row.get("commit_seq"),
    })
}

pub(crate) fn material_row(row: SqliteRow) -> Result<Material> {
    Ok(Material {
        id: MaterialId(row.get("id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        content_hash: row.get("content_hash"),
        media_type: row.get("media_type"),
        byte_len: row.get("byte_len"),
        source: row.get("source"),
        event_time: time_point(&row, "event_time_domain", "event_time_coordinate"),
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
    })
}

pub(crate) fn observation_row(row: SqliteRow) -> Result<Observation> {
    Ok(Observation {
        id: ObservationId(row.get("id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        material_id: MaterialId(row.get("material_id")),
        observed_at: time_point(&row, "observed_at_domain", "observed_at_coordinate"),
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
    })
}

fn time_point(row: &SqliteRow, domain: &str, coordinate: &str) -> Option<TimePoint> {
    let domain: Option<String> = row.get(domain);
    domain.map(|domain| TimePoint {
        domain,
        coordinate: row.get(coordinate),
    })
}
