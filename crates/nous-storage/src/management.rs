//! Memory lifecycle commits, source metadata, and content-free purge markers.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Purge transaction shape adapted from Nous memory/src/service/lifecycle.rs,
// commit 412b062082afb75ccfae4c3f69fc606d3e3cc862; see LICENSE.nous and NOTICE.
// Modified for SQLite revision snapshots, durable apalis work and JSON receipts.
use nous_domain::{
    Error, ErrorCode, MaterialDetails, MaterialId, Memory, MemoryAudit, MemoryId, NamespaceId,
    Result, SubjectId,
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, sqlite::SqliteRow};

use crate::{
    Database, WriteTransaction, cognition::memory_row, not_found, storage_error, write_error,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurgedMemory {
    pub id: MemoryId,
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub revision: i64,
    pub commit_seq: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenancePage {
    pub memories: Vec<Memory>,
    pub next_cursor: Option<MemoryId>,
    pub snapshot_seq: i64,
    pub has_more: bool,
}

impl Database {
    pub async fn recycle_bin(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let limit = positive_limit(limit)?;
        sqlx::query("SELECT r.* FROM memory_revisions r JOIN memories m ON m.id=r.memory_id AND m.current_revision=r.revision WHERE m.namespace_id=? AND m.subject_id=? AND r.deleted=1 ORDER BY r.commit_seq,r.memory_id LIMIT ?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(limit)
            .fetch_all(&self.pool).await.map_err(storage_error)?.into_iter().map(memory_row).collect()
    }

    /// Traverse stable identity at a fixed birth boundary. Each page returns
    /// current state; the application rechecks eligibility in its write transaction.
    pub async fn maintenance_page(
        &self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        after: Option<&MemoryId>,
        snapshot_seq: Option<i64>,
        limit: usize,
    ) -> Result<MaintenancePage> {
        let fetch_limit = positive_limit(limit)?
            .checked_add(1)
            .ok_or_else(|| Error::invalid("maintenance page limit is out of range"))?;
        let mut tx = self.pool.begin().await.map_err(storage_error)?;
        let upper: i64 =
            sqlx::query_scalar("SELECT commit_seq FROM authority_state WHERE singleton=1")
                .fetch_one(&mut *tx)
                .await
                .map_err(storage_error)?;
        let snapshot_seq = snapshot_seq.unwrap_or(upper);
        if snapshot_seq < 0 || snapshot_seq > upper {
            return Err(Error::new(
                ErrorCode::Conflict,
                "maintenance snapshot must refer to committed authority",
            ));
        }
        let mut memories=sqlx::query("SELECT r.* FROM memories m JOIN memory_revisions r ON r.memory_id=m.id AND r.revision=m.current_revision WHERE m.namespace_id=? AND m.subject_id=? AND (? IS NULL OR m.id>?) AND EXISTS(SELECT 1 FROM memory_revisions birth WHERE birth.memory_id=m.id AND birth.commit_seq<=?) ORDER BY m.id LIMIT ?")
            .bind(namespace.as_str()).bind(subject.as_str()).bind(after.map(MemoryId::as_str)).bind(after.map(MemoryId::as_str)).bind(snapshot_seq).bind(fetch_limit)
            .fetch_all(&mut *tx).await.map_err(storage_error)?.into_iter().map(memory_row).collect::<Result<Vec<_>>>()?;
        let has_more = memories.len() > limit;
        if has_more {
            memories.pop();
        }
        let next_cursor = if has_more {
            memories.last().map(|memory| memory.id.clone())
        } else {
            None
        };
        tx.commit().await.map_err(storage_error)?;
        Ok(MaintenancePage {
            memories,
            next_cursor,
            snapshot_seq,
            has_more,
        })
    }

    pub async fn memory_audit(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
        limit: usize,
    ) -> Result<Vec<MemoryAudit>> {
        sqlx::query("SELECT * FROM memory_audit WHERE namespace_id=? AND memory_id=? ORDER BY commit_seq DESC,id DESC LIMIT ?")
            .bind(namespace.as_str()).bind(id.as_str()).bind(positive_limit(limit)?)
            .fetch_all(&self.pool).await.map_err(storage_error)?.into_iter().map(audit_row).collect()
    }

    pub async fn material_details(
        &self,
        namespace: &NamespaceId,
        id: &MaterialId,
    ) -> Result<MaterialDetails> {
        let row = sqlx::query("SELECT d.source_type,d.tags,d.metadata,d.structured FROM materials m LEFT JOIN material_details d ON d.material_id=m.id WHERE m.namespace_id=? AND m.id=?")
            .bind(namespace.as_str()).bind(id.as_str()).fetch_optional(&self.pool).await.map_err(storage_error)?.ok_or_else(not_found)?;
        Ok(MaterialDetails {
            source_type: row.get("source_type"),
            tags: serde_json::from_str(row.get::<Option<&str>, _>("tags").unwrap_or("[]"))
                .map_err(storage_error)?,
            metadata: serde_json::from_str(row.get::<Option<&str>, _>("metadata").unwrap_or("{}"))
                .map_err(storage_error)?,
            structured: row
                .get::<Option<&str>, _>("structured")
                .map(serde_json::from_str)
                .transpose()
                .map_err(storage_error)?,
        })
    }

    pub async fn purged_memory(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
    ) -> Result<PurgedMemory> {
        sqlx::query("SELECT * FROM memory_purges WHERE namespace_id=? AND memory_id=?")
            .bind(namespace.as_str())
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(purge_row)
            .transpose()?
            .ok_or_else(not_found)
    }
}

impl WriteTransaction {
    pub async fn append_memory_audit(&mut self, audit: &MemoryAudit) -> Result<()> {
        self.check_sequence(audit.commit_seq)?;
        let owner = sqlx::query("SELECT namespace_id,subject_id FROM memories WHERE id=? UNION ALL SELECT namespace_id,subject_id FROM memory_purges WHERE memory_id=? LIMIT 1")
            .bind(audit.memory_id.as_str()).bind(audit.memory_id.as_str()).fetch_optional(&mut *self.tx).await.map_err(storage_error)?.ok_or_else(not_found)?;
        if owner.get::<String, _>("namespace_id") != audit.namespace_id.as_str()
            || owner.get::<String, _>("subject_id") != audit.subject_id.as_str()
        {
            return Err(Error::invalid(
                "memory audit scope does not match its memory",
            ));
        }
        sqlx::query("INSERT INTO memory_audit(id,namespace_id,subject_id,memory_id,action,actor,reason,previous_revision,revision,recorded_at,commit_seq,details) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&audit.id).bind(audit.namespace_id.as_str()).bind(audit.subject_id.as_str()).bind(audit.memory_id.as_str()).bind(&audit.action).bind(&audit.actor).bind(&audit.reason)
            .bind(audit.previous_revision).bind(audit.revision).bind(&audit.recorded_at).bind(audit.commit_seq).bind(serde_json::to_string(&audit.details).map_err(storage_error)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    pub async fn set_material_details(
        &mut self,
        namespace: &NamespaceId,
        id: &MaterialId,
        details: &MaterialDetails,
    ) -> Result<()> {
        self.material(namespace, id).await?;
        sqlx::query("INSERT INTO material_details(material_id,namespace_id,source_type,tags,metadata,structured) VALUES (?,?,?,?,?,?) ON CONFLICT(material_id) DO UPDATE SET source_type=excluded.source_type,tags=excluded.tags,metadata=excluded.metadata,structured=excluded.structured")
            .bind(id.as_str()).bind(namespace.as_str()).bind(&details.source_type).bind(serde_json::to_string(&details.tags).map_err(storage_error)?)
            .bind(serde_json::to_string(&details.metadata).map_err(storage_error)?).bind(details.structured.as_ref().map(serde_json::to_string).transpose().map_err(storage_error)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(())
    }

    /// Permanently erase Memory-owned content while preserving minimal scope,
    /// invalidation and audit identity. Original Materials retain their owner.
    pub async fn purge_memory(
        &mut self,
        namespace: &NamespaceId,
        id: &MemoryId,
        expected_revision: i64,
    ) -> Result<PurgedMemory> {
        let memory = self.memory(namespace, id).await?;
        if memory.revision != expected_revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "memory revision has changed",
            ));
        }
        if !memory.deleted {
            return Err(Error::new(
                ErrorCode::Conflict,
                "memory must be in the recycle bin before permanent purge",
            ));
        }
        let revision = memory
            .revision
            .checked_add(1)
            .ok_or_else(|| Error::invalid("memory revision is out of range"))?;
        let marker = PurgedMemory {
            id: id.clone(),
            namespace_id: namespace.clone(),
            subject_id: memory.subject_id,
            revision,
            commit_seq: self.commit_seq(),
        };
        sqlx::query("UPDATE model_calls SET response=? WHERE id IN (SELECT DISTINCT model_call_id FROM memory_revisions WHERE memory_id=?)")
            .bind(r#"{"schema_version":1,"redacted":true,"reason":"memory_purged"}"#).bind(id.as_str())
            .execute(&mut *self.tx).await.map_err(write_error)?;
        sqlx::query("UPDATE operation_receipts SET receipt=json_set(receipt,'$.result',json_object('id',?,'purged',json('true'))) WHERE json_extract(receipt,'$.result.id')=?")
            .bind(id.as_str()).bind(id.as_str()).execute(&mut *self.tx).await.map_err(write_error)?;
        sqlx::query("DELETE FROM formation_memories WHERE memory_id=?")
            .bind(id.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        sqlx::query("DELETE FROM memory_revisions WHERE memory_id=?")
            .bind(id.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        sqlx::query("DELETE FROM memories WHERE id=? AND namespace_id=?")
            .bind(id.as_str())
            .bind(namespace.as_str())
            .execute(&mut *self.tx)
            .await
            .map_err(write_error)?;
        sqlx::query("INSERT INTO memory_purges(memory_id,namespace_id,subject_id,revision,commit_seq) VALUES (?,?,?,?,?)")
            .bind(id.as_str()).bind(namespace.as_str()).bind(marker.subject_id.as_str()).bind(marker.revision).bind(marker.commit_seq)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        Ok(marker)
    }
}

pub(crate) fn purge_row(row: SqliteRow) -> Result<PurgedMemory> {
    Ok(PurgedMemory {
        id: MemoryId(row.get("memory_id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        revision: row.get("revision"),
        commit_seq: row.get("commit_seq"),
    })
}

fn audit_row(row: SqliteRow) -> Result<MemoryAudit> {
    Ok(MemoryAudit {
        id: row.get("id"),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        memory_id: MemoryId(row.get("memory_id")),
        action: row.get("action"),
        actor: row.get("actor"),
        reason: row.get("reason"),
        previous_revision: row.get("previous_revision"),
        revision: row.get("revision"),
        recorded_at: row.get("recorded_at"),
        commit_seq: row.get("commit_seq"),
        details: serde_json::from_str(row.get("details")).map_err(storage_error)?,
    })
}

fn positive_limit(limit: usize) -> Result<i64> {
    i64::try_from(limit)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::invalid("limit must be a positive integer"))
}
