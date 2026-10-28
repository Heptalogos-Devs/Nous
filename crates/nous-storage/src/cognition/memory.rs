use crate::codec::*;
use crate::{Database, WriteTransaction, not_found, storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection, sqlite::SqliteRow};

impl Database {
    pub async fn memory_namespace(&self, id: &MemoryId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM memories WHERE id=? UNION ALL SELECT namespace_id FROM memory_purges WHERE memory_id=? LIMIT 1")
            .bind(id.as_str())
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn observation_namespace(&self, id: &ObservationId) -> Result<NamespaceId> {
        sqlx::query_scalar::<_, String>("SELECT namespace_id FROM observations WHERE id=?")
            .bind(id.as_str())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(NamespaceId)
            .ok_or_else(not_found)
    }

    pub async fn memory(&self, namespace: &NamespaceId, id: &MemoryId) -> Result<Memory> {
        memory(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
        )
        .await
    }

    pub async fn memory_history(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
    ) -> Result<Vec<Memory>> {
        let rows = sqlx::query(
            "SELECT * FROM memory_revisions WHERE namespace_id=? AND memory_id=? ORDER BY revision",
        )
        .bind(namespace.as_str())
        .bind(id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        if rows.is_empty() {
            return Err(not_found());
        }
        rows.into_iter().map(memory_row).collect()
    }

    pub async fn memory_revision(
        &self,
        namespace: &NamespaceId,
        id: &MemoryId,
        revision: i64,
    ) -> Result<Memory> {
        revision_in(
            &mut *self.pool.acquire().await.map_err(storage_error)?,
            namespace,
            id,
            revision,
        )
        .await
    }
}

async fn revision_in(
    conn: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MemoryId,
    revision: i64,
) -> Result<Memory> {
    if revision <= 0 {
        return Err(Error::invalid(
            "Memory source requires a positive exact revision",
        ));
    }
    sqlx::query(
        "SELECT * FROM memory_revisions WHERE namespace_id=? AND memory_id=? AND revision=?",
    )
    .bind(namespace.as_str())
    .bind(id.as_str())
    .bind(revision)
    .fetch_optional(conn)
    .await
    .map_err(storage_error)?
    .map(memory_row)
    .transpose()?
    .ok_or_else(not_found)
}

impl WriteTransaction {
    pub async fn memory(&mut self, namespace: &NamespaceId, id: &MemoryId) -> Result<Memory> {
        memory(&mut self.tx, namespace, id).await
    }

    pub async fn memory_revision(
        &mut self,
        namespace: &NamespaceId,
        id: &MemoryId,
        revision: i64,
    ) -> Result<Memory> {
        revision_in(&mut self.tx, namespace, id, revision).await
    }

    /// `None` creates revision 1. `Some(n)` appends revision n+1 and moves the
    /// current pointer only when n is still current; all older snapshots remain.
    pub async fn save_memory(
        &mut self,
        value: &Memory,
        expected_revision: Option<i64>,
    ) -> Result<()> {
        self.check_sequence(value.commit_seq)?;
        self.validate_memory_source(
            &value.namespace_id,
            &value.subject_id,
            &value.observation_id,
            &value.material_id,
            &value.source,
        )
        .await?;
        let has_model = value.model_call_id.is_some();
        let has_neither = value.model_call_id.is_none();
        if match &value.origin {
            MemoryOrigin::Model => !has_model,
            MemoryOrigin::Imported | MemoryOrigin::Manual => !has_neither,
        } {
            return Err(Error::invalid(
                "model Memory requires its actual call; imported/manual Memory has no formation call",
            ));
        }
        if !value.lifecycle.strength.is_finite() || !(0.0..=1.0).contains(&value.lifecycle.strength)
        {
            return Err(Error::invalid(
                "memory retention strength must be finite and within [0, 1]",
            ));
        }
        let revision = expected_revision
            .unwrap_or(0)
            .checked_add(1)
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::invalid("memory revision is out of range"))?;
        if value.revision != revision {
            return Err(Error::invalid(
                "memory revision must follow expected_revision",
            ));
        }
        match expected_revision {
            None => {
                let was_purged: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM memory_purges WHERE memory_id=?)",
                )
                .bind(value.id.as_str())
                .fetch_one(&mut *self.tx)
                .await
                .map_err(storage_error)?;
                if was_purged {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "permanently purged memory identity cannot be reused",
                    ));
                }
                sqlx::query("INSERT INTO memories(id,namespace_id,subject_id,current_revision,commit_seq) VALUES (?,?,?,?,?)")
                    .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(value.commit_seq)
                    .execute(&mut *self.tx).await.map_err(write_error)?;
            }
            Some(expected) => {
                let changed = sqlx::query("UPDATE memories SET current_revision=?,commit_seq=? WHERE id=? AND namespace_id=? AND subject_id=? AND current_revision=?")
                    .bind(value.revision).bind(value.commit_seq).bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(expected)
                    .execute(&mut *self.tx).await.map_err(write_error)?.rows_affected();
                if changed == 0 {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "memory revision or scope has changed",
                    ));
                }
            }
        }
        sqlx::query("INSERT INTO memory_revisions(memory_id,namespace_id,subject_id,revision,title,content,epistemic_status,observation_id,material_id,source,model_call_id,times,recorded_at,deleted,commit_seq,lifecycle_strength,lifecycle_anchor_unix_ms,lifecycle_activation,lifecycle_pinned,lifecycle_protected_until_unix_ms,lifecycle_inactive_since_unix_ms,lifecycle_last_reinforced_unix_ms,origin) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(value.id.as_str()).bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.revision).bind(&value.title).bind(&value.content).bind(&value.epistemic_status)
            .bind(value.observation_id.as_str()).bind(value.material_id.as_str()).bind(serde_json::to_string(&value.source)?).bind(value.model_call_id.as_ref().map(ModelCallId::as_str))
            .bind(serde_json::to_string(&value.times).map_err(storage_error)?).bind(&value.recorded_at).bind(value.deleted).bind(value.commit_seq)
            .bind(value.lifecycle.strength).bind(value.lifecycle.anchor_unix_ms).bind(enum_name(&value.lifecycle.activation)?).bind(value.lifecycle.pinned)
            .bind(value.lifecycle.protected_until_unix_ms).bind(value.lifecycle.inactive_since_unix_ms).bind(value.lifecycle.last_reinforced_unix_ms)
            .bind(enum_name(&value.origin)?)
            .execute(&mut *self.tx).await.map_err(write_error)?;
        self.ensure_retrieval_addresses(
            &value.namespace_id,
            &RetrievalTarget::Cognition(CognitiveRef::memory(&value.id, value.revision)),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn validate_memory_source(
        &mut self,
        namespace: &NamespaceId,
        subject: &SubjectId,
        observation: &ObservationId,
        material: &MaterialId,
        source: &media::MediaReference,
    ) -> Result<()> {
        let observed = self.observation(namespace, observation).await?;
        if observed.subject_id != *subject || observed.material_id != *material {
            return Err(Error::invalid(
                "memory source event differs from its subject or material",
            ));
        }
        let roots = self.media_roots(namespace, subject, source).await?;
        if !roots.contains(material) {
            return Err(Error::invalid(
                "memory source does not contain its observation material",
            ));
        }
        for root in roots {
            if !self
                .subject_has_material_access(namespace, subject, &root)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "memory source root is not accessible to this subject",
                ));
            }
        }
        Ok(())
    }
}

pub(crate) async fn memory(
    connection: &mut SqliteConnection,
    namespace: &NamespaceId,
    id: &MemoryId,
) -> Result<Memory> {
    memory_row(sqlx::query("SELECT r.* FROM memory_revisions r JOIN memories m ON m.id=r.memory_id AND m.current_revision=r.revision WHERE m.namespace_id=? AND m.id=?")
        .bind(namespace.as_str()).bind(id.as_str()).fetch_optional(connection).await.map_err(storage_error)?.ok_or_else(not_found)?)
}

pub(crate) fn memory_row(row: SqliteRow) -> Result<Memory> {
    memory_row_ref(&row)
}

pub(crate) fn memory_row_ref(row: &SqliteRow) -> Result<Memory> {
    Ok(Memory {
        id: MemoryId(row.get("memory_id")),
        namespace_id: NamespaceId(row.get("namespace_id")),
        subject_id: SubjectId(row.get("subject_id")),
        revision: row.get("revision"),
        title: row.get("title"),
        content: row.get("content"),
        epistemic_status: row.get("epistemic_status"),
        observation_id: ObservationId(row.get("observation_id")),
        material_id: nous_domain::MaterialId(row.get("material_id")),
        source: serde_json::from_str(row.get("source"))?,
        model_call_id: row
            .get::<Option<String>, _>("model_call_id")
            .map(ModelCallId),
        times: serde_json::from_str(row.get("times")).map_err(storage_error)?,
        recorded_at: row.get("recorded_at"),
        deleted: row.get("deleted"),
        commit_seq: row.get("commit_seq"),
        origin: parse_enum(row.get("origin"))?,
        lifecycle: MemoryLifecycle {
            strength: row.get("lifecycle_strength"),
            anchor_unix_ms: row.get("lifecycle_anchor_unix_ms"),
            activation: parse_enum(row.get("lifecycle_activation"))?,
            pinned: row.get("lifecycle_pinned"),
            protected_until_unix_ms: row.get("lifecycle_protected_until_unix_ms"),
            inactive_since_unix_ms: row.get("lifecycle_inactive_since_unix_ms"),
            last_reinforced_unix_ms: row.get("lifecycle_last_reinforced_unix_ms"),
        },
    })
}

impl crate::ReadView {
    pub async fn memory(&mut self, ns: &NamespaceId, id: &MemoryId) -> Result<Memory> {
        memory(&mut self.tx, ns, id).await
    }
}
