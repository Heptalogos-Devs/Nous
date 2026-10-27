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
    material_row_ref(&row, "")
}

pub(crate) fn material_row_ref(row: &SqliteRow, prefix: &'static str) -> Result<Material> {
    let column = |name: &str| format!("{prefix}{name}");
    Ok(Material {
        id: MaterialId(row.get(column("id").as_str())),
        namespace_id: NamespaceId(row.get(column("namespace_id").as_str())),
        content_hash: row.get(column("content_hash").as_str()),
        media_type: row.get(column("media_type").as_str()),
        metadata: serde_json::from_str(row.get(column("metadata").as_str()))
            .map_err(storage_error)?,
        byte_len: row.get(column("byte_len").as_str()),
        source: row.get(column("source").as_str()),
        event_time: time_point(
            row,
            &column("event_time_domain"),
            &column("event_time_coordinate"),
        ),
        recorded_at: row.get(column("recorded_at").as_str()),
        commit_seq: row.get(column("commit_seq").as_str()),
    })
}

pub(crate) fn observation_row(row: SqliteRow) -> Result<Observation> {
    Ok(Observation {
        id: ObservationId(row.get("id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        material_id: MaterialId(row.get("material_id")),
        kind: serde_json::from_value(serde_json::Value::String(row.get("kind")))
            .map_err(storage_error)?,
        observed_at: time_point(&row, "observed_at_domain", "observed_at_coordinate"),
        origin: row
            .get::<Option<&str>, _>("origin")
            .map(serde_json::from_str)
            .transpose()?,
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

impl crate::ReadView {
    pub async fn subject_namespace(&mut self, id: &SubjectId) -> nous_domain::Result<NamespaceId> {
        subject_namespace(&mut self.tx, id).await
    }
    pub async fn namespace(&mut self, id: &NamespaceId) -> nous_domain::Result<Namespace> {
        namespace(&mut self.tx, id)
            .await?
            .ok_or_else(crate::not_found)
    }
    pub async fn subject(
        &mut self,
        ns: &NamespaceId,
        id: &SubjectId,
    ) -> nous_domain::Result<Subject> {
        subject(&mut self.tx, ns, id)
            .await?
            .ok_or_else(crate::not_found)
    }
    pub async fn material(
        &mut self,
        ns: &NamespaceId,
        id: &MaterialId,
    ) -> nous_domain::Result<Material> {
        material(&mut self.tx, ns, id)
            .await?
            .ok_or_else(crate::not_found)
    }
    pub async fn observation(
        &mut self,
        ns: &NamespaceId,
        id: &ObservationId,
    ) -> nous_domain::Result<Observation> {
        observation(&mut self.tx, ns, id)
            .await?
            .ok_or_else(crate::not_found)
    }
}
pub(crate) async fn subject_namespace(
    conn: &mut SqliteConnection,
    id: &SubjectId,
) -> nous_domain::Result<NamespaceId> {
    sqlx::query_scalar::<_, String>("SELECT namespace_id FROM subjects WHERE id=?")
        .bind(id.as_str())
        .fetch_optional(conn)
        .await
        .map_err(storage_error)?
        .map(NamespaceId)
        .ok_or_else(crate::not_found)
}
