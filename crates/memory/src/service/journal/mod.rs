//! Supported narrative Authority over settled Episode organization.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
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
        let row = sqlx::query("SELECT o.*,r.journal_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.temporal_scope_kind,r.temporal_scope_start,r.temporal_scope_end,r.narrative,r.formed_at,r.recorded_at,r.producer_signature_id FROM journal_objects o JOIN journal_revisions r ON r.journal_id=o.journal_id AND r.journal_revision_id=COALESCE($3,o.current_revision_id) WHERE o.subject_id=$1 AND o.journal_id=$2")
            .bind(subject.0).bind(journal.0).bind(revision.map(|id| id.0)).fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("Journal not found".into()))?;
        let revision = JournalRevisionId(row.try_get("journal_revision_id").map_err(db)?);
        let point_rows = sqlx::query("SELECT p.ordinal,p.role,p.text,COALESCE(jsonb_agg(s.basis ORDER BY s.basis_no) FILTER(WHERE s.basis_no IS NOT NULL),'[]') AS basis FROM journal_revision_points p LEFT JOIN journal_point_basis s USING(journal_revision_id,ordinal) WHERE p.journal_revision_id=$1 GROUP BY p.ordinal,p.role,p.text ORDER BY p.ordinal")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let points = point_rows
            .into_iter()
            .map(|row| {
                Ok(JournalPoint {
                    role: parse_enum(row.try_get("role").map_err(db)?, "Journal point role")?,
                    text: row.try_get("text").map_err(db)?,
                    basis: serde_json::from_value(row.try_get("basis").map_err(db)?)
                        .map_err(|error| Error::Infrastructure(error.to_string()))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let source_rows = sqlx::query("SELECT ref_kind,ref_value FROM journal_revision_sources WHERE journal_revision_id=$1 ORDER BY ref_kind,ref_value")
            .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)?;
        let sources = source_rows
            .into_iter()
            .map(|row| {
                parse_reference(
                    &row.get::<String, _>("ref_kind"),
                    &row.get::<String, _>("ref_value"),
                )
            })
            .collect::<Result<_>>()?;
        Ok(JournalView {
            object: JournalObject {
                journal_id: journal,
                subject_id: subject,
                current_revision_id: JournalRevisionId(
                    row.try_get("current_revision_id").map_err(db)?,
                ),
                object_epoch: row.try_get("object_epoch").map_err(db)?,
                acceptance_state: parse_enum(
                    row.try_get("acceptance_state").map_err(db)?,
                    "acceptance state",
                )?,
                integrity_state: parse_enum(
                    row.try_get("integrity_state").map_err(db)?,
                    "integrity state",
                )?,
                suppression_state: parse_enum(
                    row.try_get("suppression_state").map_err(db)?,
                    "suppression state",
                )?,
                created_at: row.try_get("created_at").map_err(db)?,
            },
            revision: JournalRevision {
                journal_revision_id: revision,
                journal_id: journal,
                subject_id: subject,
                revision_no: row.try_get("revision_no").map_err(db)?,
                parent_revision_id: row
                    .try_get::<Option<Uuid>, _>("parent_revision_id")
                    .map_err(db)?
                    .map(JournalRevisionId),
                revision_intent: row.try_get("revision_intent").map_err(db)?,
                title: row.try_get("title").map_err(db)?,
                temporal_scope: temporal_from_columns(
                    row.try_get("temporal_scope_kind").map_err(db)?,
                    row.try_get("temporal_scope_start").map_err(db)?,
                    row.try_get("temporal_scope_end").map_err(db)?,
                )?,
                narrative: row.try_get("narrative").map_err(db)?,
                formed_at: row.try_get("formed_at").map_err(db)?,
                recorded_at: row.try_get("recorded_at").map_err(db)?,
                producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
            },
            points,
            sources,
        })
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
            JournalId(id.ok_or_else(|| Error::NotFound("Journal revision not found".into()))?),
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
            return Err(Error::Invalid("Journal page limit is out of bounds".into()));
        }
        if !["", "accepted", "withdrawn"].contains(&status) {
            return Err(Error::Invalid("invalid Journal status filter".into()));
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
