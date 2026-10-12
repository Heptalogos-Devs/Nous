//! Supported narrative Authority over settled Episode organization.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
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
        let selected: Option<Uuid> = sqlx::query_scalar("SELECT r.journal_revision_id FROM journal_objects o JOIN journal_revisions r ON r.journal_id=o.journal_id AND r.journal_revision_id=COALESCE($3,o.current_revision_id) WHERE o.subject_id=$1 AND o.journal_id=$2")
            .bind(subject.0).bind(journal.0).bind(revision.map(|id| id.0)).fetch_optional(self.store.pool()).await.map_err(db)?;
        self.journal_revision(
            subject,
            JournalRevisionId(selected.ok_or_else(|| Error::NotFound("Journal not found".into()))?),
        )
        .await
    }

    async fn journal_views(
        &self,
        subject: SubjectId,
        revisions: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, JournalView>> {
        use std::collections::HashMap;
        let rows = sqlx::query("SELECT o.*,r.journal_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.temporal_scope_kind,r.temporal_scope_start,r.temporal_scope_end,r.narrative,r.formed_at,r.recorded_at,r.producer_signature_id FROM journal_objects o JOIN journal_revisions r ON r.journal_id=o.journal_id WHERE o.subject_id=$1 AND r.journal_revision_id=ANY($2)").bind(subject.0).bind(revisions).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut points: HashMap<Uuid, Vec<JournalPoint>> = HashMap::new();
        for row in sqlx::query("SELECT p.journal_revision_id,p.ordinal,p.role,p.text,COALESCE(jsonb_agg(s.basis ORDER BY s.basis_no) FILTER(WHERE s.basis_no IS NOT NULL),'[]') AS basis FROM journal_revision_points p LEFT JOIN journal_point_basis s USING(journal_revision_id,ordinal) WHERE p.journal_revision_id=ANY($1) GROUP BY p.journal_revision_id,p.ordinal,p.role,p.text ORDER BY p.journal_revision_id,p.ordinal")
            .bind(revisions).fetch_all(self.store.pool()).await.map_err(db)? {
            points.entry(row.try_get("journal_revision_id").map_err(db)?).or_default().push(JournalPoint {
                role: parse_enum(row.try_get("role").map_err(db)?, "Journal point role")?,
                text: row.try_get("text").map_err(db)?,
                basis: serde_json::from_value(row.try_get("basis").map_err(db)?).map_err(|error| Error::Infrastructure(error.to_string()))?,
            });
        }
        let mut sources: HashMap<Uuid, Vec<CognitiveRef>> = HashMap::new();
        for row in sqlx::query("SELECT journal_revision_id,ref_kind,ref_value FROM journal_revision_sources WHERE journal_revision_id=ANY($1) ORDER BY journal_revision_id,ref_kind,ref_value")
            .bind(revisions).fetch_all(self.store.pool()).await.map_err(db)? {
            sources.entry(row.try_get("journal_revision_id").map_err(db)?).or_default().push(parse_reference(&row.try_get::<String,_>("ref_kind").map_err(db)?, &row.try_get::<String,_>("ref_value").map_err(db)?)?);
        }
        rows.into_iter()
            .map(|row| {
                let journal = JournalId(row.try_get("journal_id").map_err(db)?);
                let revision = JournalRevisionId(row.try_get("journal_revision_id").map_err(db)?);
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
                    points: points.remove(&revision.0).unwrap_or_default(),
                    sources: sources.remove(&revision.0).unwrap_or_default(),
                })
            })
            .map(|result: Result<JournalView>| {
                result.map(|view| (view.revision.journal_revision_id.0, view))
            })
            .collect()
    }

    pub async fn journal_revision(
        &self,
        subject: SubjectId,
        revision: JournalRevisionId,
    ) -> Result<JournalView> {
        self.journal_views(subject, &[revision.0])
            .await?
            .remove(&revision.0)
            .ok_or_else(|| Error::NotFound("Journal revision not found".into()))
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
        if !matches!(status, "" | "accepted" | "withdrawn") {
            return Err(Error::Invalid("invalid Journal status filter".into()));
        }
        self.store.require_subject(subject).await?;
        let ids: Vec<Uuid> = if let Some(journal) = journal {
            self.store
                .validate_reference(subject, &CognitiveRef::Journal(journal))
                .await?;
            let after_no = if let Some(last) = last {
                Some(sqlx::query_scalar::<_, i32>("SELECT revision_no FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2 AND journal_revision_id=$3")
                    .bind(subject.0).bind(journal.0).bind(last).fetch_optional(self.store.pool()).await.map_err(db)?
                    .ok_or_else(|| Error::Invalid("invalid Journal history cursor".into()))?)
            } else {
                None
            };
            sqlx::query_scalar("SELECT journal_revision_id FROM journal_revisions WHERE subject_id=$1 AND journal_id=$2 AND ($3::int IS NULL OR revision_no>$3) ORDER BY revision_no LIMIT $4")
                .bind(subject.0).bind(journal.0).bind(after_no).bind(limit).fetch_all(self.store.pool()).await.map_err(db)?
        } else {
            sqlx::query_scalar("SELECT current_revision_id FROM journal_objects WHERE subject_id=$1 AND ($2::uuid IS NULL OR journal_id>$2) AND ($4='' OR acceptance_state=$4) ORDER BY journal_id LIMIT $3")
                .bind(subject.0).bind(last).bind(limit).bind(status).fetch_all(self.store.pool()).await.map_err(db)?
        };
        let mut views = self.journal_views(subject, &ids).await?;
        ids.into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("Journal page changed while reading".into()))
            })
            .collect()
    }
}

impl MemoryService {
    pub(in crate::service) async fn wake_journal_revalidation_in(
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
