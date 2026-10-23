// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use crate::codec::*;
use crate::{Database, WriteTransaction, not_found, storage_error};
use nous_domain::*;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredSnapshot {
    pub watermark: i64,
    pub objects: Vec<StructuredObject>,
    pub complete: bool,
}

impl Database {
    pub async fn structured_namespace(&self, id: &CognitiveId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM structured_objects WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn structured_object(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let object = object(&mut tx, namespace, id, None).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(object)
    }

    pub async fn structured_revision(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: i64,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let object = object(&mut tx, namespace, id, Some(revision)).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(object)
    }

    pub async fn structured_history(
        &self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<Vec<StructuredObject>> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let revisions: Vec<i64> = sqlx::query_scalar("SELECT revision FROM structured_revisions WHERE namespace_id=? AND object_id=? ORDER BY revision")
            .bind(namespace.as_str()).bind(id.as_str()).fetch_all(&mut *tx).await.map_err(storage_error)?;
        if revisions.is_empty() {
            return Err(not_found());
        }
        let mut history = Vec::with_capacity(revisions.len());
        for revision in revisions {
            history.push(object(&mut tx, namespace, id, Some(revision)).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(history)
    }

    pub async fn subject_structured_snapshot(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        kind: Option<CognitiveKind>,
        as_of_commit: Option<i64>,
        limit: usize,
    ) -> Result<StructuredSnapshot> {
        let fetch_limit = i64::try_from(limit)
            .ok()
            .filter(|v| *v > 0 && *v <= 10000)
            .and_then(|v| v.checked_add(1))
            .ok_or_else(|| Error::invalid("structured snapshot limit must be 1..10000"))?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let current: i64 =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let watermark = as_of_commit.unwrap_or(current);
        if watermark < 0 || watermark > current {
            return Err(Error::invalid("snapshot refers to uncommitted authority"));
        }
        let kind = kind.as_ref().map(enum_name).transpose()?;
        let rows=sqlx::query("SELECT r.* FROM structured_revisions r WHERE r.namespace_id=? AND r.subject_id=? AND (? IS NULL OR r.kind=?) AND r.revision=(SELECT max(v.revision) FROM structured_revisions v WHERE v.object_id=r.object_id AND v.commit_seq<=?) ORDER BY r.object_id LIMIT ?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(&kind).bind(&kind).bind(watermark).bind(fetch_limit).fetch_all(&mut *tx).await.map_err(storage_error)?;
        let complete = rows.len() <= limit;
        let mut objects = Vec::with_capacity(rows.len().min(limit));
        for row in rows.into_iter().take(limit) {
            objects.push(decode(&mut tx, row).await?);
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(StructuredSnapshot {
            watermark,
            objects,
            complete,
        })
    }

    pub async fn concept_by_alias(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        alias: &str,
    ) -> Result<StructuredObject> {
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let id:String=sqlx::query_scalar("SELECT concept_id FROM concept_aliases WHERE namespace_id=? AND subject_id=? AND alias_key=?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(alias.trim().to_lowercase()).fetch_optional(&mut *tx).await.map_err(storage_error)?.ok_or_else(not_found)?;
        let result = object(&mut tx, namespace, &CognitiveId(id), None).await?;
        tx.commit().await.map_err(storage_error)?;
        Ok(result)
    }
}

impl WriteTransaction {
    pub async fn structured_object(
        &mut self,
        namespace: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, namespace, id, None).await
    }

    pub async fn structured_revision(
        &mut self,
        namespace: &NamespaceId,
        id: &CognitiveId,
        revision: i64,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, namespace, id, Some(revision)).await
    }
}

pub(crate) async fn object(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &CognitiveId,
    revision: Option<i64>,
) -> Result<StructuredObject> {
    let row=sqlx::query("SELECT r.* FROM structured_revisions r JOIN structured_objects o ON o.id=r.object_id WHERE r.namespace_id=? AND r.object_id=? AND r.revision=coalesce(?,o.current_revision)")
        .bind(namespace.as_str()).bind(id.as_str()).bind(revision).fetch_optional(&mut *conn).await.map_err(storage_error)?.ok_or_else(not_found)?;
    decode(conn, row).await
}
pub(crate) async fn decode(
    conn: &mut SqliteConnection,
    row: SqliteRow,
) -> Result<StructuredObject> {
    let id: String = row.get("object_id");
    let revision: i64 = row.get("revision");
    let links=sqlx::query("SELECT * FROM structured_links WHERE object_id=? AND revision=? AND category='evidence' ORDER BY ordinal")
        .bind(&id).bind(revision).fetch_all(conn).await.map_err(storage_error)?;
    let evidence = links
        .into_iter()
        .map(|link| {
            Ok(EvidenceLink {
                source: crate::source::references::source_row(&link)?,
                polarity: parse_enum(link.get("polarity"))?,
                relation: link
                    .get::<Option<&str>, _>("relation")
                    .map(parse_enum)
                    .transpose()?,
                explanation: link.get("explanation"),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(StructuredObject {
        id: CognitiveId(id),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        revision,
        body: serde_json::from_str(row.get("body"))?,
        evidence,
        times: serde_json::from_str(row.get("times"))?,
        producer_call_id: row
            .get::<Option<String>, _>("producer_call_id")
            .map(nous_domain::ModelCallId),
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
        deleted: row.get("deleted"),
    })
}

impl crate::ReadView {
    pub async fn structured_object(
        &mut self,
        ns: &NamespaceId,
        id: &CognitiveId,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, ns, id, None).await
    }
    pub async fn structured_revision(
        &mut self,
        ns: &NamespaceId,
        id: &CognitiveId,
        revision: i64,
    ) -> Result<StructuredObject> {
        object(&mut self.tx, ns, id, Some(revision)).await
    }
}
