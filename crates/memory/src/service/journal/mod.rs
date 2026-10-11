//! Supported narrative Authority over settled Episode organization.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;
mod lifecycle;
mod mutations;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPointRole {
    Summary,
    Outcome,
    Change,
    Decision,
    OpenQuestion,
    SalientEvent,
    Reflection,
}

impl JournalPointRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Outcome => "outcome",
            Self::Change => "change",
            Self::Decision => "decision",
            Self::OpenQuestion => "open_question",
            Self::SalientEvent => "salient_event",
            Self::Reflection => "reflection",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalPoint {
    pub role: JournalPointRole,
    pub text: String,
    pub basis: Vec<RevisionBasis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalTarget {
    pub journal_id: JournalId,
    pub expected_revision: JournalRevisionId,
    pub expected_epoch: i64,
    pub intent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub expected_authority_seq: i64,
    pub target: Option<JournalTarget>,
    pub sources: Vec<EpisodePartitionSource>,
    pub title: Option<String>,
    pub narrative: String,
    pub points: Vec<JournalPoint>,
    pub producer: Option<ProducerSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalObject {
    pub journal_id: JournalId,
    pub subject_id: SubjectId,
    pub current_revision_id: JournalRevisionId,
    pub object_epoch: i64,
    pub acceptance_state: AcceptanceState,
    pub integrity_state: IntegrityState,
    pub suppression_state: SuppressionState,
    pub purge_state: PurgeState,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRevision {
    pub journal_revision_id: JournalRevisionId,
    pub journal_id: JournalId,
    pub subject_id: SubjectId,
    pub revision_no: i32,
    pub parent_revision_id: Option<JournalRevisionId>,
    pub revision_intent: Option<String>,
    pub title: Option<String>,
    pub temporal_scope: TemporalExtent,
    pub narrative: String,
    pub formed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer_signature_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalView {
    pub object: JournalObject,
    pub revision: JournalRevision,
    pub points: Vec<JournalPoint>,
    pub sources: Vec<CognitiveRef>,
}

impl MemoryService {
    pub async fn journal(
        &self,
        subject: SubjectId,
        journal: JournalId,
        revision: Option<JournalRevisionId>,
    ) -> Result<JournalView> {
        let reference = match revision {
            Some(revision) => CognitiveRef::JournalRevision(revision),
            None => CognitiveRef::Journal(journal),
        };
        let view = self
            .journals_by_references(subject, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(ErrorOwner::Memory, ErrorCode::NotFound, "Journal not found")
            })?;
        if view.object.journal_id != journal {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::NotFound,
                "Journal not found",
            ));
        }
        Ok(view)
    }

    /// Read complete Journal owner views for current object and exact revision
    /// references using grouped object/revision, point, basis and source reads.
    pub async fn journals_by_references(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<HashMap<CognitiveRef, JournalView>> {
        if references.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Memory,
                ErrorCode::InvalidRequest,
                "Journal context batch exceeds 256 references",
            ));
        }
        let journal_ids = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::Journal(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        let requested_revisions = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::JournalRevision(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        if journal_ids.is_empty() && requested_revisions.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let rows = sqlx::query("SELECT o.journal_id,o.subject_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,o.created_at,r.journal_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.temporal_scope_kind,r.temporal_scope_start,r.temporal_scope_end,r.narrative,r.formed_at,r.recorded_at,r.producer_signature_id FROM journal_objects o JOIN journal_revisions r ON r.journal_id=o.journal_id AND ((o.journal_id=ANY($2) AND r.journal_revision_id=o.current_revision_id) OR r.journal_revision_id=ANY($3)) WHERE o.subject_id=$1 ORDER BY o.journal_id,r.revision_no")
            .bind(subject.0)
            .bind(&journal_ids)
            .bind(&requested_revisions)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let revision_ids = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("journal_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        if revision_ids.is_empty() {
            tx.commit().await.map_err(db)?;
            return Ok(HashMap::new());
        }

        let mut points = HashMap::<Uuid, Vec<JournalPoint>>::new();
        for row in sqlx::query("SELECT p.journal_revision_id,p.ordinal,p.role,p.text,COALESCE(jsonb_agg(s.basis ORDER BY s.basis_no) FILTER(WHERE s.basis_no IS NOT NULL),'[]') AS basis FROM journal_revision_points p LEFT JOIN journal_point_basis s USING(journal_revision_id,ordinal) WHERE p.journal_revision_id=ANY($1) GROUP BY p.journal_revision_id,p.ordinal,p.role,p.text ORDER BY p.journal_revision_id,p.ordinal")
            .bind(&revision_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?
        {
            let revision: Uuid = row.try_get("journal_revision_id").map_err(db)?;
            let role = parse_enum(
                ErrorOwner::Memory,
                row.try_get("role").map_err(db)?,
                "Journal point role",
            )?;
            let basis = serde_json::from_value(row.try_get("basis").map_err(db)?).map_err(
                |error| {
                    Error::new(
                        ErrorOwner::Memory,
                        ErrorCode::InternalFailure,
                        "Could not decode a persisted journal point basis",
                    )
                    .with_source(error)
                },
            )?;
            points.entry(revision).or_default().push(JournalPoint {
                role,
                text: row.try_get("text").map_err(db)?,
                basis,
            });
        }
        let mut sources = HashMap::<Uuid, Vec<CognitiveRef>>::new();
        for row in sqlx::query("SELECT journal_revision_id,ref_kind,ref_value FROM journal_revision_sources WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ref_kind,ref_value")
            .bind(&revision_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?
        {
            let revision: Uuid = row.try_get("journal_revision_id").map_err(db)?;
            sources.entry(revision).or_default().push(parse_reference(
                &row.try_get::<String, _>("ref_kind").map_err(db)?,
                &row.try_get::<String, _>("ref_value").map_err(db)?,
            )?);
        }
        tx.commit().await.map_err(db)?;

        let mut by_journal = HashMap::<JournalId, JournalView>::new();
        let mut by_revision = HashMap::<JournalRevisionId, JournalView>::new();
        for row in rows {
            let journal_id = JournalId(row.try_get("journal_id").map_err(db)?);
            let revision_id = JournalRevisionId(row.try_get("journal_revision_id").map_err(db)?);
            let view = JournalView {
                object: JournalObject {
                    journal_id,
                    subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
                    current_revision_id: JournalRevisionId(
                        row.try_get("current_revision_id").map_err(db)?,
                    ),
                    object_epoch: row.try_get("object_epoch").map_err(db)?,
                    acceptance_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("acceptance_state").map_err(db)?,
                        "acceptance state",
                    )?,
                    integrity_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("integrity_state").map_err(db)?,
                        "integrity state",
                    )?,
                    suppression_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("suppression_state").map_err(db)?,
                        "suppression state",
                    )?,
                    purge_state: parse_enum(
                        ErrorOwner::Memory,
                        row.try_get("purge_state").map_err(db)?,
                        "purge state",
                    )?,
                    created_at: row.try_get("created_at").map_err(db)?,
                },
                revision: JournalRevision {
                    journal_revision_id: revision_id,
                    journal_id,
                    subject_id: subject,
                    revision_no: row.try_get("revision_no").map_err(db)?,
                    parent_revision_id: row
                        .try_get::<Option<Uuid>, _>("parent_revision_id")
                        .map_err(db)?
                        .map(JournalRevisionId),
                    revision_intent: row.try_get("revision_intent").map_err(db)?,
                    title: row.try_get("title").map_err(db)?,
                    temporal_scope: temporal_from_columns(
                        ErrorOwner::Memory,
                        row.try_get("temporal_scope_kind").map_err(db)?,
                        row.try_get("temporal_scope_start").map_err(db)?,
                        row.try_get("temporal_scope_end").map_err(db)?,
                    )?,
                    narrative: row.try_get("narrative").map_err(db)?,
                    formed_at: row.try_get("formed_at").map_err(db)?,
                    recorded_at: row.try_get("recorded_at").map_err(db)?,
                    producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
                },
                points: points.remove(&revision_id.0).unwrap_or_default(),
                sources: sources.remove(&revision_id.0).unwrap_or_default(),
            };
            if view.object.current_revision_id == revision_id {
                by_journal.insert(journal_id, view.clone());
            }
            by_revision.insert(revision_id, view);
        }
        let mut result = HashMap::new();
        for reference in references {
            let view = match reference {
                CognitiveRef::Journal(id) => by_journal.get(id).cloned(),
                CognitiveRef::JournalRevision(id) => by_revision.get(id).cloned(),
                _ => None,
            };
            if let Some(view) = view {
                result.insert(reference.clone(), view);
            }
        }
        Ok(result)
    }

    pub async fn journal_revision(
        &self,
        subject: SubjectId,
        revision: JournalRevisionId,
    ) -> Result<JournalView> {
        let id: Option<Uuid> = sqlx::query_scalar("SELECT journal_id FROM journal_revisions WHERE subject_id=$1 AND journal_revision_id=$2")
            .bind(subject.0).bind(revision.0).fetch_optional(self.store.pool()).await.map_err(db)?;
        self.journal(
            subject,
            JournalId(id.ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::NotFound,
                    "Journal revision not found",
                )
            })?),
            Some(revision),
        )
        .await
    }

    pub async fn journal_page(
        &self,
        subject: SubjectId,
        journal: Option<JournalId>,
        limit: i64,
        last: Option<Uuid>,
        status: &str,
    ) -> Result<Vec<JournalView>> {
        if !(1..=201).contains(&limit) {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Journal page limit is out of bounds",
            ));
        }
        if !["", "accepted", "withdrawn"].contains(&status) {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid Journal status filter",
            ));
        }
        self.store.require_subject(subject).await?;
        if let Some(journal) = journal {
            let ids: Vec<Uuid> = sqlx::query_scalar("SELECT journal_revision_id FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2 AND ($3::uuid IS NULL OR journal_revision_id>$3) ORDER BY journal_revision_id LIMIT $4")
                .bind(subject.0).bind(journal.0).bind(last).bind(limit).fetch_all(self.store.pool()).await.map_err(db)?;
            let mut views = Vec::with_capacity(ids.len());
            for revision in ids {
                views.push(
                    self.journal(subject, journal, Some(JournalRevisionId(revision)))
                        .await?,
                );
            }
            Ok(views)
        } else {
            let ids: Vec<Uuid> = sqlx::query_scalar("SELECT journal_id FROM journal_objects WHERE subject_id=$1 AND ($2::uuid IS NULL OR journal_id>$2) AND ($4='' OR acceptance_state=$4) ORDER BY journal_id LIMIT $3")
                .bind(subject.0).bind(last).bind(limit).bind(status).fetch_all(self.store.pool()).await.map_err(db)?;
            let mut views = Vec::with_capacity(ids.len());
            for journal in ids {
                views.push(self.journal(subject, JournalId(journal), None).await?);
            }
            Ok(views)
        }
    }

    pub async fn list_journals(&self, subject: SubjectId) -> Result<Vec<JournalView>> {
        self.store.require_subject(subject).await?;
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT journal_id FROM journal_objects WHERE subject_id=$1 ORDER BY created_at,journal_id LIMIT 256")
            .bind(subject.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut views = Vec::with_capacity(ids.len());
        for id in ids {
            views.push(self.journal(subject, JournalId(id), None).await?);
        }
        Ok(views)
    }

    pub async fn journal_history(
        &self,
        subject: SubjectId,
        journal: JournalId,
    ) -> Result<Vec<JournalView>> {
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT journal_revision_id FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2 ORDER BY revision_no LIMIT 256")
            .bind(subject.0).bind(journal.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut views = Vec::with_capacity(ids.len());
        for id in ids {
            views.push(
                self.journal(subject, journal, Some(JournalRevisionId(id)))
                    .await?,
            );
        }
        Ok(views)
    }
}

impl MemoryService {
    async fn wake_journal_revalidation_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        journal: JournalId,
        sequence: i64,
    ) -> Result<()> {
        self.cognition
            .wake_blocked_maintenance_in(
                tx,
                &nous_runtime::MaintenanceRequest {
                    subject,
                    kind: "journal_revalidate".into(),
                    scope_kind: "journal".into(),
                    scope_ref: journal.0.to_string(),
                    trigger_authority_seq: sequence,
                    due_at: self.cognition.now(subject),
                    priority: 30,
                },
            )
            .await
    }
}
