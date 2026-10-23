use crate::{Database, ReadView, SourceClosure, SourceStatus, codec::*, storage_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRetrievalEntry {
    pub memory: Memory,
    pub material: Material,
    pub details: MaterialDetails,
    pub embedding: Option<EmbeddingRecord>,
}
struct MemorySnapshot {
    watermark: i64,
    entries: Vec<MemoryRetrievalEntry>,
    tag_embeddings: Vec<EmbeddingRecord>,
    complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootMaterial {
    pub material: Material,
    pub details: MaterialDetails,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredRetrievalEntry {
    pub object: StructuredObject,
    pub embedding: Option<EmbeddingRecord>,
    pub source: SourceClosure,
    pub base_memory: Option<Memory>,
    pub materials: Vec<RootMaterial>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveRetrievalSnapshot {
    pub watermark: i64,
    pub memories: Vec<MemoryRetrievalEntry>,
    pub structured: Vec<StructuredRetrievalEntry>,
    pub tag_embeddings: Vec<EmbeddingRecord>,
    pub complete: bool,
}
enum Selection<'a> {
    Scope(usize),
    Exact(&'a CognitiveRef),
}
impl Database {
    pub async fn cognitive_retrieval_snapshot(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        as_of_commit: Option<i64>,
        limit: usize,
    ) -> Result<CognitiveRetrievalSnapshot> {
        let mut view = self.read_view().await?;
        let result = view
            .cognitive_retrieval_snapshot(namespace, subject, space_id, as_of_commit, limit)
            .await?;
        view.finish().await?;
        Ok(result)
    }
    pub async fn cognitive_reference_snapshot(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        reference: &CognitiveRef,
        as_of_commit: Option<i64>,
    ) -> Result<CognitiveRetrievalSnapshot> {
        let mut view = self.read_view().await?;
        let result = view
            .cognitive_reference_snapshot(namespace, subject, space_id, reference, as_of_commit)
            .await?;
        view.finish().await?;
        Ok(result)
    }
}
impl ReadView {
    pub async fn cognitive_retrieval_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        as_of_commit: Option<i64>,
        limit: usize,
    ) -> Result<CognitiveRetrievalSnapshot> {
        snapshot_in(
            &mut self.tx,
            namespace,
            subject,
            space_id,
            as_of_commit,
            Selection::Scope(limit),
        )
        .await
    }
    pub async fn cognitive_reference_snapshot(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        space_id: &str,
        reference: &CognitiveRef,
        as_of_commit: Option<i64>,
    ) -> Result<CognitiveRetrievalSnapshot> {
        snapshot_in(
            &mut self.tx,
            namespace,
            subject,
            space_id,
            as_of_commit,
            Selection::Exact(reference),
        )
        .await
    }
}
async fn snapshot_in(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space_id: &str,
    as_of_commit: Option<i64>,
    selection: Selection<'_>,
) -> Result<CognitiveRetrievalSnapshot> {
    let (limit, reference) = match selection {
        Selection::Scope(limit) => (limit, None),
        Selection::Exact(reference) => {
            reference.validate()?;
            let sequence =
                crate::source::references::validate_reference(conn, namespace, subject, reference)
                    .await?;
            if as_of_commit.is_some_and(|upper| sequence > upper) {
                return Err(crate::not_found());
            }
            (1, Some(reference))
        }
    };
    if limit == 0 || limit > 10000 {
        return Err(Error::invalid(
            "cognitive retrieval snapshot limit must be 1..10000",
        ));
    }

    let memory = memory_snapshot_in(
        conn,
        namespace,
        subject,
        space_id,
        limit,
        as_of_commit,
        reference,
    )
    .await?;
    let remaining = limit.saturating_sub(memory.entries.len());
    let rows=sqlx::query("SELECT o.id,(SELECT max(r.revision) FROM structured_revisions r WHERE r.object_id=o.id AND r.commit_seq<=? AND (? IS NULL OR r.revision=?)) AS selected_revision FROM structured_objects o WHERE o.namespace_id=? AND o.subject_id=? AND selected_revision IS NOT NULL AND (? IS NULL OR o.id=?) ORDER BY o.kind,o.id LIMIT ?")
            .bind(memory.watermark).bind(reference.map(|r|r.revision)).bind(reference.map(|r|r.revision)).bind(namespace.as_str()).bind(subject.as_str()).bind(reference.map(|r| if r.kind == CognitiveKind::Memory { "" } else {r.id.as_str()})).bind(reference.map(|r| if r.kind == CognitiveKind::Memory { "" } else {r.id.as_str()})).bind((remaining+1) as i64).fetch_all(&mut *conn).await.map_err(storage_error)?;
    let complete = memory.complete && rows.len() <= remaining;
    let records=sqlx::query("SELECT e.* FROM embedding_records e WHERE e.namespace_id=? AND e.subject_id=? AND e.space_id=? AND e.target_kind='cognition'")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).fetch_all(&mut *conn).await.map_err(storage_error)?.into_iter().map(super::records::row).collect::<Result<Vec<_>>>()?;
    let mut by_key = records
        .into_iter()
        .map(|record| Ok((record.source_key()?, record)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut structured = Vec::with_capacity(rows.len().min(remaining));
    let mut tag_keys = BTreeSet::new();
    for row in rows.into_iter().take(remaining) {
        let object = crate::cognition::structured::read::object(
            conn,
            namespace,
            &CognitiveId(row.get("id")),
            Some(row.get("selected_revision")),
        )
        .await?;
        let reference = object.reference();
        let source = crate::source::closure::closure_at(
            conn,
            namespace,
            subject,
            reference.clone(),
            as_of_commit,
        )
        .await?;
        let base_memory = super::source::base_memory(conn, &object).await?;
        let mut root_materials = Vec::new();
        for root in &source.roots {
            if root.status == SourceStatus::Missing {
                continue;
            }
            let Some(material) =
                crate::authority::records::material(conn, namespace, &root.material_id).await?
            else {
                continue;
            };
            let details = material_details(conn, namespace, &root.material_id).await?;
            for tag in &details.tags {
                let tag = tag.trim();
                if !tag.is_empty() {
                    tag_keys.insert(EmbeddingInputItem::tag(tag)?.source_hash());
                }
            }
            root_materials.push(RootMaterial { material, details });
        }
        structured.push(StructuredRetrievalEntry {
            embedding: by_key.remove(&cognitive_embedding_key(&reference)?),
            object,
            source,
            base_memory,
            materials: root_materials,
        });
    }
    let mut tag_embeddings = memory
        .tag_embeddings
        .into_iter()
        .map(|record| Ok((record.source_key()?, record)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let rows=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND target_kind='tag' AND source_hash IN (SELECT value FROM json_each(?)) ORDER BY source_hash")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).bind(serde_json::to_string(&tag_keys)?).fetch_all(&mut *conn).await.map_err(storage_error)?;
    for row in rows {
        let record = super::records::row(row)?;
        tag_embeddings.insert(record.source_key()?, record);
    }

    Ok(CognitiveRetrievalSnapshot {
        watermark: memory.watermark,
        memories: memory.entries,
        structured,
        tag_embeddings: tag_embeddings.into_values().collect(),
        complete,
    })
}
async fn memory_snapshot_in(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    subject: &SubjectId,
    space_id: &str,
    limit: usize,
    as_of_commit: Option<i64>,
    reference: Option<&CognitiveRef>,
) -> Result<MemorySnapshot> {
    let limit = i64::try_from(limit)
        .ok()
        .filter(|v| *v > 0)
        .and_then(|v| v.checked_add(1))
        .ok_or_else(|| Error::invalid("snapshot limit must be positive"))?;
    let upper: i64 = sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
        .fetch_one(&mut *conn)
        .await
        .map_err(storage_error)?;
    let watermark = as_of_commit.unwrap_or(upper);
    if watermark < 0 || watermark > upper {
        return Err(Error::invalid(
            "retrieval cutoff refers to uncommitted authority",
        ));
    }
    let mut rows=sqlx::query("SELECT r.*,s.content_hash AS source_hash,s.media_type AS source_media_type,s.byte_len AS source_byte_len,s.source AS source_name,s.event_time_domain AS source_event_domain,s.event_time_coordinate AS source_event_coordinate,s.recorded_at AS source_recorded_at,s.commit_seq AS source_commit_seq,d.source_type AS detail_source_type,d.tags AS detail_tags,d.metadata AS detail_metadata,d.structured AS detail_structured FROM memories m JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=(SELECT max(h.revision) FROM memory_revisions h WHERE h.memory_id=m.id AND h.commit_seq<=? AND (? IS NULL OR h.revision=?)) JOIN materials s ON s.id=r.material_id LEFT JOIN material_details d ON d.material_id=s.id WHERE m.namespace_id=? AND m.subject_id=? AND (? IS NULL OR m.id=?) ORDER BY m.id LIMIT ?")
            .bind(watermark).bind(reference.map(|r|r.revision)).bind(reference.map(|r|r.revision)).bind(namespace.as_str()).bind(subject.as_str()).bind(reference.map(|r| if r.kind == CognitiveKind::Memory {r.id.as_str()} else { "" })).bind(reference.map(|r| if r.kind == CognitiveKind::Memory {r.id.as_str()} else { "" })).bind(limit).fetch_all(&mut *conn).await.map_err(storage_error)?;
    let complete = rows.len() < limit as usize;
    if !complete {
        rows.pop();
    }
    let mut entries = Vec::with_capacity(rows.len());
    let mut tags = BTreeSet::new();
    for row in rows {
        let memory = crate::cognition::memory::memory_row_ref(&row)?;
        let domain: Option<String> = row.get("source_event_domain");
        let material = Material {
            id: memory.material_id.clone(),
            namespace_id: namespace.clone(),
            content_hash: row.get("source_hash"),
            media_type: row.get("source_media_type"),
            byte_len: row.get("source_byte_len"),
            source: row.get("source_name"),
            event_time: domain.map(|domain| TimePoint {
                domain,
                coordinate: row.get("source_event_coordinate"),
            }),
            recorded_at: row.get("source_recorded_at"),
            commit_seq: row.get("source_commit_seq"),
        };
        let details = MaterialDetails {
            source_type: row.get("detail_source_type"),
            tags: serde_json::from_str(row.get::<Option<&str>, _>("detail_tags").unwrap_or("[]"))
                .map_err(storage_error)?,
            metadata: serde_json::from_str(
                row.get::<Option<&str>, _>("detail_metadata")
                    .unwrap_or("{}"),
            )
            .map_err(storage_error)?,
            structured: parse_optional(row.get("detail_structured"))?,
        };
        for tag in &details.tags {
            let text = tag.trim();
            if !text.is_empty() {
                tags.insert(EmbeddingInputItem::tag(text)?.source_hash());
            }
        }
        entries.push(MemoryRetrievalEntry {
            memory,
            material,
            details,
            embedding: None,
        });
    }
    let ids = entries
        .iter()
        .map(|entry| entry.memory.id.to_string())
        .collect::<Vec<_>>();
    let records=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND target_kind='cognition' AND cognition_kind='memory' AND cognition_id IN(SELECT value FROM json_each(?))").bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).bind(serde_json::to_string(&ids)?).fetch_all(&mut *conn).await.map_err(storage_error)?;
    let mut by_key = records
        .into_iter()
        .map(super::records::row)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|e| Ok((e.source_key()?, e)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for entry in &mut entries {
        entry.embedding = by_key.remove(&cognitive_embedding_key(&CognitiveRef::memory(
            &entry.memory.id,
            entry.memory.revision,
        ))?);
    }
    let tag_embeddings=sqlx::query("SELECT * FROM embedding_records WHERE namespace_id=? AND subject_id=? AND space_id=? AND target_kind='tag' AND source_hash IN (SELECT value FROM json_each(?)) ORDER BY source_hash")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(space_id).bind(serde_json::to_string(&tags)?).fetch_all(&mut *conn).await.map_err(storage_error)?
            .into_iter().map(super::records::row).collect::<Result<Vec<_>>>()?;
    Ok(MemorySnapshot {
        watermark,
        entries,
        tag_embeddings,
        complete,
    })
}
async fn material_details(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MaterialId,
) -> Result<MaterialDetails> {
    let row=sqlx::query("SELECT source_type,tags,metadata,structured FROM material_details WHERE namespace_id=? AND material_id=?")
        .bind(namespace.as_str()).bind(id.as_str()).fetch_optional(conn).await.map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(MaterialDetails::default());
    };
    Ok(MaterialDetails {
        source_type: row.get("source_type"),
        tags: serde_json::from_str(row.get("tags"))?,
        metadata: serde_json::from_str(row.get("metadata"))?,
        structured: parse_optional_json(row.get("structured"))?,
    })
}
fn parse_optional_json(value: Option<&str>) -> Result<Option<serde_json::Value>> {
    value
        .map(serde_json::from_str)
        .transpose()
        .map_err(storage_error)
}
