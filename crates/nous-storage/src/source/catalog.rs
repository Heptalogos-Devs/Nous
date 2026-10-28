use crate::{ReadView, WriteTransaction, authority::records, storage_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};
use std::collections::BTreeMap;

/// Current record membership; raw input containers do not create additional memberships.
pub struct SourceMemoryEntry {
    pub id: MemoryId,
    pub revision: i64,
    pub material_id: MaterialId,
    pub observation_id: ObservationId,
    pub source: Option<String>,
    pub protected: bool,
    pub recorded_at: String,
}
async fn entries(
    conn: &mut SqliteConnection,
    ns: &NamespaceId,
    sub: &SubjectId,
    now: i64,
) -> Result<Vec<SourceMemoryEntry>> {
    let rows=sqlx::query("SELECT r.memory_id,r.revision,r.material_id,r.observation_id,r.lifecycle_pinned,r.lifecycle_protected_until_unix_ms,r.recorded_at FROM memories m JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=m.current_revision WHERE m.namespace_id=? AND m.subject_id=? AND r.deleted=0 ORDER BY r.memory_id")
        .bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *conn).await.map_err(storage_error)?;
    let mut names = BTreeMap::new();
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let material_id = MaterialId(row.get("material_id"));
        if !names.contains_key(&material_id) {
            let material = records::material(conn, ns, &material_id)
                .await?
                .ok_or_else(crate::not_found)?;
            let details = super::materials::material_details(conn, ns, &material_id).await?;
            names.insert(
                material_id.clone(),
                material_source_name(&material, &details).map(str::to_owned),
            );
        }
        result.push(SourceMemoryEntry {
            id: MemoryId(row.get("memory_id")),
            revision: row.get("revision"),
            source: names[&material_id].clone(),
            material_id,
            observation_id: ObservationId(row.get("observation_id")),
            protected: row.get::<bool, _>("lifecycle_pinned")
                || row
                    .get::<Option<i64>, _>("lifecycle_protected_until_unix_ms")
                    .is_some_and(|until| until > now),
            recorded_at: row.get("recorded_at"),
        });
    }
    Ok(result)
}
impl ReadView {
    pub async fn source_memory_entries(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
        now: i64,
    ) -> Result<Vec<SourceMemoryEntry>> {
        entries(&mut self.tx, ns, sub, now).await
    }
}
impl WriteTransaction {
    pub async fn evidence_source_status(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
        source: &EvidenceSource,
    ) -> Result<crate::SourceStatus> {
        let (status, _, _) =
            super::closure::source_node(&mut self.tx, ns, sub, source, false, None).await?;
        Ok(status)
    }
    pub async fn source_memory_entries(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
    ) -> Result<Vec<SourceMemoryEntry>> {
        let now = self.time().unix_ms();
        entries(&mut self.tx, ns, sub, now).await
    }
    pub async fn source_graph_objects(
        &mut self,
        ns: &NamespaceId,
        sub: &SubjectId,
    ) -> Result<Vec<StructuredObject>> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT o.id FROM structured_objects o JOIN structured_revisions r ON r.object_id=o.id AND r.revision=o.current_revision WHERE o.namespace_id=? AND o.subject_id=? AND r.deleted=0 AND o.kind IN('entity','association','memory_semantics') ORDER BY o.kind,o.id")
            .bind(ns.as_str()).bind(sub.as_str()).fetch_all(&mut *self.tx).await.map_err(storage_error)?;
        let mut result = Vec::with_capacity(ids.len());
        for id in ids {
            result.push(self.structured_object(ns, &CognitiveId(id)).await?);
        }
        Ok(result)
    }
}
