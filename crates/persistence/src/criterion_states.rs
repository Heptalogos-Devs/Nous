// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use chrono::{DateTime, Utc};
use nous_core::*;
use sqlx::Row;
/// Fixed owner catalogs supply table identifiers. This exposes complete recorded
/// state metadata, never prose or a query supplied by the evaluator caller.
impl AuthorityStore {
    pub async fn motivation_source_edges_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        revision: uuid::Uuid,
    ) -> Result<Vec<(String, String)>> {
        sqlx::query_as("SELECT d.ref_kind,d.ref_value FROM motivation_dependencies d JOIN motivation_revisions r USING(revision_id) WHERE r.subject_id=$1 AND d.revision_id=$2").bind(subject.0).bind(revision).fetch_all(&mut **tx).await.map_err(database_error)
    }

    pub async fn cognition_state_history_in(
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        reference: &CognitiveRef,
        through: DateTime<Utc>,
        domain: &str,
    ) -> Result<Vec<(DateTime<Utc>, serde_json::Value)>> {
        let capability = match reference.domain() {
            CognitiveDomain::Memory => "memory",
            CognitiveDomain::SelfCognition => "self_cognition",
            CognitiveDomain::SocialCognition => "social_cognition",
            CognitiveDomain::Motivation => "motivation",
            CognitiveDomain::Runtime => "runtime",
            _ => return Err(Error::Invalid("Unsupported owner state selector".into())),
        };
        let enabled:bool=sqlx::query_scalar("SELECT CASE $2 WHEN 'memory' THEN memory WHEN 'self_cognition' THEN self_cognition WHEN 'social_cognition' THEN social_cognition WHEN 'motivation' THEN motivation ELSE true END FROM subject_capabilities WHERE subject_id=$1").bind(subject.0).bind(capability).fetch_optional(&mut **tx).await.map_err(database_error)?.unwrap_or(false);
        if !enabled {
            return Err(Error::Unavailable("State owner is unavailable".into()));
        }
        let (table, col, kind) = match reference {
            CognitiveRef::WorkContext(_) => ("work_contexts", "work_context_id", "work_context"),
            CognitiveRef::MemoryRevision(_) => ("memory_revisions", "memory_id", "memory"),
            CognitiveRef::CognitiveSchemaRevision(_) => (
                "cognitive_schema_revisions",
                "schema_id",
                "cognitive_schema",
            ),
            CognitiveRef::EpisodeRevision(_) => ("episode_revisions", "episode_id", "episode"),
            CognitiveRef::JournalRevision(_) => ("journal_revisions", "journal_id", "journal"),
            r if r.domain() == CognitiveDomain::SelfCognition && r.is_cognition_revision() => {
                ("self_revisions", "object_id", "self")
            }
            r if r.domain() == CognitiveDomain::SocialCognition && r.is_cognition_revision() => {
                ("social_revisions", "object_id", "social")
            }
            r if r.domain() == CognitiveDomain::Motivation && r.is_cognition_revision() => {
                ("motivation_revisions", "object_id", "motivation")
            }
            _ => {
                return Err(Error::Invalid(
                    "ownerState requires a supported exact owner revision".into(),
                ));
            }
        };
        let revision_col = match reference {
            CognitiveRef::WorkContext(_) => "work_context_id",
            CognitiveRef::MemoryRevision(_) => "memory_revision_id",
            CognitiveRef::CognitiveSchemaRevision(_) => "schema_revision_id",
            CognitiveRef::EpisodeRevision(_) => "episode_revision_id",
            CognitiveRef::JournalRevision(_) => "journal_revision_id",
            _ => "revision_id",
        };
        let introduced = if matches!(reference, CognitiveRef::WorkContext(_)) {
            "created_at"
        } else {
            "recorded_at"
        };
        let introduced_world = match reference {
            CognitiveRef::WorkContext(_) => "(SELECT (first.semantic_stamp->>'at')::timestamptz FROM authority_object_states first WHERE first.subject_id=r.subject_id AND first.object_kind=$3 AND first.object_ref=r.work_context_id::text ORDER BY first.event_id LIMIT 1)".to_owned(),
            r if r.domain() == CognitiveDomain::Motivation => "r.formed_at".to_owned(),
            _ => "r.formed_at".to_owned(),
        };
        let introduced_domain = match reference {
            CognitiveRef::WorkContext(_) => {
                "(SELECT first.semantic_stamp->>'time_domain_ref' FROM authority_object_states first WHERE first.subject_id=r.subject_id AND first.object_kind=$3 AND first.object_ref=r.work_context_id::text ORDER BY first.event_id LIMIT 1)"
            }
            r if r.domain() == CognitiveDomain::Motivation => "r.formed_stamp->>'time_domain_ref'",
            _ => "r.formed_time_domain_ref",
        };
        let text = format!(
            "SELECT CASE WHEN $5='time:real' THEN GREATEST(h.recorded_at,r.{introduced}) ELSE GREATEST((h.semantic_stamp->>'at')::timestamptz,{introduced_world}) END AS recorded_at,h.state FROM {table} r JOIN authority_object_states h ON h.subject_id=r.subject_id AND h.object_ref=r.{col}::text AND h.object_kind=$3 WHERE r.subject_id=$1 AND r.{revision_col}=$2 AND r.{introduced}<=clock_timestamp() AND (($5='time:real' AND h.recorded_at<=$4 AND r.{introduced}<=$4) OR ($5<>'time:real' AND {introduced_domain}=$5 AND h.semantic_stamp->>'time_domain_ref'=$5 AND (h.semantic_stamp->>'at')::timestamptz<=$4 AND {introduced_world}<=$4 AND NOT EXISTS(SELECT 1 FROM authority_object_states gap WHERE gap.subject_id=h.subject_id AND gap.object_kind=h.object_kind AND gap.object_ref=h.object_ref AND gap.event_id>h.event_id AND gap.semantic_stamp->>'time_domain_ref'<>$5))) ORDER BY h.recorded_at,h.event_id LIMIT 4097"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(text.as_str()))
            .bind(subject.0)
            .bind(reference.uuid())
            .bind(kind)
            .bind(through)
            .bind(domain)
            .fetch_all(&mut **tx)
            .await
            .map_err(database_error)?;
        if rows.len() > 4096 {
            return Err(Error::Unavailable(
                "owner state history exceeds bounded read".into(),
            ));
        }
        rows.into_iter()
            .map(|r| {
                Ok((
                    r.try_get("recorded_at").map_err(database_error)?,
                    r.try_get("state").map_err(database_error)?,
                ))
            })
            .collect()
    }
}
