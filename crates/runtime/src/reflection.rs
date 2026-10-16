//! Bounded reflection opportunities over actual captured experience, never inbox.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_configuration::*;
use nous_persistence::database_error as db;
use sqlx::{Postgres, Row, Transaction};
pub const REFLECTION_DOMAINS: ConfigKey<Vec<String>> = ConfigKey::new("reflection.enabled_domains");
pub const REFLECTION_DEBOUNCE: ConfigKey<u64> = ConfigKey::new("reflection.debounce_seconds");
pub const REFLECTION_SOURCES: ConfigKey<u64> = ConfigKey::new("reflection.source_limit");
pub const REFLECTION_SOURCE_BYTES: ConfigKey<u64> = ConfigKey::new("reflection.source_max_bytes");
pub const REFLECTION_CANDIDATES: ConfigKey<u64> = ConfigKey::new("reflection.candidate_limit");
pub const REFLECTION_CANDIDATE_BYTES: ConfigKey<u64> =
    ConfigKey::new("reflection.candidate_max_bytes");
pub const REFLECTION_ACTIONS: ConfigKey<u64> = ConfigKey::new("reflection.max_actions");
pub fn register_reflection_configuration(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(REFLECTION_DOMAINS,"cognitive-runtime","Explicit Host opt-in domains for shared reflection; Motivation is unsupported.",Vec::<String>::new(),ConfigExposure::Standard,ConfigScopePolicy::SubjectOverrideAllowed,ConfigApplyMode::Live,ConfigSemanticEffect::AuthorityFormation,|v|{if v.len()<=3&&v.iter().all(|d|["memory","self","social"].contains(&d.as_str()))&&v.iter().collect::<std::collections::HashSet<_>>().len()==v.len(){Ok(())}else{Err(Error::Invalid("reflection domains must be unique implemented memory/self/social owners; Motivation is unsupported".into()))}})?;
    for (key, description, default, max) in [
        (
            REFLECTION_DEBOUNCE,
            "Delay to coalesce actual observed events before a reflection opportunity",
            15,
            3600,
        ),
        (
            REFLECTION_SOURCES,
            "Maximum actual sources in one reflection batch",
            16,
            128,
        ),
        (
            REFLECTION_SOURCE_BYTES,
            "Maximum source text bytes in one reflection batch",
            262144,
            1048576,
        ),
        (
            REFLECTION_CANDIDATES,
            "Maximum candidates per enabled owner in shared reflection",
            32,
            128,
        ),
        (
            REFLECTION_CANDIDATE_BYTES,
            "Maximum complete candidate body/support bytes per owner",
            262144,
            1048576,
        ),
        (
            REFLECTION_ACTIONS,
            "Maximum typed actions in one shared reflection",
            16,
            128,
        ),
    ] {
        registry.register(
            key,
            "cognitive-runtime",
            description,
            default,
            ConfigExposure::Advanced,
            ConfigScopePolicy::SubjectOverrideAllowed,
            ConfigApplyMode::Live,
            ConfigSemanticEffect::AuthorityFormation,
            move |v| {
                if (1..=max).contains(v) {
                    Ok(())
                } else {
                    Err(Error::Invalid(format!("{} requires 1..{max}", key.path())))
                }
            },
        )?;
    }
    Ok(())
}
pub struct ReflectionSource {
    pub reference: CognitiveRef,
    pub recorded_seq: i64,
    pub work_context: Option<WorkContextId>,
    pub work_context_revision: Option<i64>,
    pub session: Option<SessionId>,
}
pub struct ReflectionSourceChange {
    pub change_id: i64,
    pub reference: CognitiveRef,
    pub reason: String,
    pub recorded_at: DateTime<Utc>,
}
pub struct ReflectionBatch {
    pub need: MaintenanceNeed,
    pub from_seq: i64,
    pub through_seq: i64,
    pub sources: Vec<ReflectionSource>,
    pub more: bool,
    pub changes: Vec<ReflectionSourceChange>,
    pub through_change: i64,
    pub domains: Vec<String>,
    pub source_bytes: u64,
    pub candidate_limit: u64,
    pub candidate_bytes: u64,
    pub max_actions: u64,
}
impl CognitiveRuntimeService {
    pub async fn reflection_domains(&self, subject: SubjectId) -> Result<Vec<String>> {
        let config = self.configuration.snapshot_for_subject(subject)?;
        let domains = config.get(REFLECTION_DOMAINS)?;
        let row=sqlx::query("SELECT memory,self_cognition,social_cognition FROM subject_capabilities WHERE subject_id=$1").bind(subject.0).fetch_one(self.store.pool()).await.map_err(db)?;
        Ok(domains
            .into_iter()
            .filter(|d| match d.as_str() {
                "memory" => row.get("memory"),
                "self" => row.get("self_cognition"),
                "social" => row.get("social_cognition"),
                _ => false,
            })
            .collect())
    }
    pub(crate) async fn trigger_reflection_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        recorded_seq: i64,
    ) -> Result<()> {
        if self
            .configuration
            .snapshot_for_subject(subject)?
            .get(REFLECTION_DOMAINS)?
            .is_empty()
        {
            return Ok(());
        }
        let domains = self.reflection_domains(subject).await?;
        if domains.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO subject_reflection_progress(subject_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(subject.0)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
        let delay = self
            .configuration
            .snapshot_for_subject(subject)?
            .get(REFLECTION_DEBOUNCE)?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("subject-reflection:{}", subject.0))
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        self.enqueue_maintenance_in(
            tx,
            &MaintenanceRequest {
                subject,
                kind: "subject_reflection".into(),
                scope_kind: "subject".into(),
                scope_ref: "observed_experience".into(),
                trigger_authority_seq: recorded_seq,
                due_at: self.now(subject) + chrono::Duration::seconds(delay as i64),
                priority: 40,
            },
        )
        .await?;
        Ok(())
    }
    pub async fn prepare_reflection(&self, selected: &MaintenanceNeed) -> Result<ReflectionBatch> {
        if selected.kind != "subject_reflection" {
            return Err(Error::Invalid(
                "reflection requires subject_reflection need".into(),
            ));
        }
        self.require_current_maintenance(selected).await?;
        let subject = selected.subject_id;
        let domains = self.reflection_domains(subject).await?;
        let config = self.configuration.snapshot_for_subject(subject)?;
        let limit = config.get(REFLECTION_SOURCES)?;
        let mut tx = self.store.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("subject-reflection:{}", subject.0))
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let current:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM maintenance_needs WHERE subject_id=$1 AND need_id=$2 AND trigger_revision=$3 AND state='pending')").bind(subject.0).bind(selected.need_id).bind(selected.trigger_revision as i64).fetch_one(&mut *tx).await.map_err(db)?;
        if !current {
            return Err(Error::Conflict("reflection capture trigger changed".into()));
        }

        let from:i64=sqlx::query_scalar("SELECT COALESCE((SELECT last_recorded_seq FROM subject_reflection_progress WHERE subject_id=$1),0)").bind(subject.0).fetch_one(&mut *tx).await.map_err(db)?;
        let rows=sqlx::query("SELECT recorded_seq,occurrence_id,active_work_context_id,active_work_context_revision,session_id FROM experience_items WHERE subject_id=$1 AND recorded_seq>$2 AND recorded_seq<=$3 AND source_class NOT IN ('generated','generation','simulation') ORDER BY recorded_seq LIMIT $4").bind(subject.0).bind(from).bind(selected.trigger_authority_seq).bind((limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more = rows.len() > limit as usize;
        let mut through = from;
        let mut sources = Vec::new();
        for row in rows.into_iter().take(limit as usize) {
            through = row.try_get("recorded_seq").map_err(db)?;
            sources.push(ReflectionSource {
                reference: CognitiveRef::Occurrence(OccurrenceId(
                    row.try_get("occurrence_id").map_err(db)?,
                )),
                recorded_seq: through,
                work_context: row
                    .try_get::<Option<Uuid>, _>("active_work_context_id")
                    .map_err(db)?
                    .map(WorkContextId),
                work_context_revision: row.try_get("active_work_context_revision").map_err(db)?,
                session: row
                    .try_get::<Option<Uuid>, _>("session_id")
                    .map_err(db)?
                    .map(SessionId),
            });
        }
        let from_change:i64=sqlx::query_scalar("SELECT COALESCE((SELECT last_change_id FROM subject_reflection_progress WHERE subject_id=$1),0)").bind(subject.0).fetch_one(&mut *tx).await.map_err(db)?;
        let change_limit = (limit as usize).saturating_sub(sources.len());
        let change_rows=sqlx::query("SELECT change_id,reference,reason,recorded_at FROM subject_reflection_source_changes WHERE subject_id=$1 AND change_id>$2 ORDER BY change_id LIMIT $3").bind(subject.0).bind(from_change).bind((change_limit+1) as i64).fetch_all(&mut *tx).await.map_err(db)?;
        let more_changes = change_rows.len() > change_limit;
        let mut through_change = from_change;
        let mut changes = Vec::new();
        for row in change_rows.into_iter().take(change_limit) {
            through_change = row.try_get("change_id").map_err(db)?;
            changes.push(ReflectionSourceChange {
                change_id: through_change,
                reference: serde_json::from_value(row.try_get("reference").map_err(db)?)
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
                reason: row.try_get("reason").map_err(db)?,
                recorded_at: row.try_get("recorded_at").map_err(db)?,
            });
        }
        tx.commit().await.map_err(db)?;
        Ok(ReflectionBatch {
            need: selected.clone(),
            from_seq: from,
            through_seq: through,
            sources,
            more: more || more_changes,
            changes,
            through_change,
            domains,
            source_bytes: config.get(REFLECTION_SOURCE_BYTES)?,
            candidate_limit: config.get(REFLECTION_CANDIDATES)?,
            candidate_bytes: config.get(REFLECTION_CANDIDATE_BYTES)?,
            max_actions: config.get(REFLECTION_ACTIONS)?,
        })
    }
    pub async fn finish_reflection(
        &self,
        selected: &MaintenanceNeed,
        through: i64,
        through_change: i64,
        operation_key: &str,
        status: &str,
        problem: Option<&str>,
    ) -> Result<()> {
        if selected.kind != "subject_reflection"
            || through < 0
            || through > selected.trigger_authority_seq
            || operation_key.len() > 512
            || !["applied", "kept", "partial", "failed", "blocked"].contains(&status)
        {
            return Err(Error::Invalid("invalid reflection completion".into()));
        }
        let mut tx = self.store.begin().await?;
        let advance = matches!(status, "applied" | "kept" | "partial" | "failed");
        sqlx::query("INSERT INTO subject_reflection_progress(subject_id,last_recorded_seq,last_operation_key,last_status,last_problem_code,updated_at) VALUES($1,CASE WHEN $2 THEN $3 ELSE 0 END,$4,$5,$6,$7) ON CONFLICT(subject_id) DO UPDATE SET last_recorded_seq=CASE WHEN $2 THEN GREATEST(subject_reflection_progress.last_recorded_seq,$3) ELSE subject_reflection_progress.last_recorded_seq END,last_operation_key=$4,last_status=$5,last_problem_code=$6,updated_at=$7").bind(selected.subject_id.0).bind(advance).bind(through).bind(operation_key).bind(status).bind(problem).bind(self.now(selected.subject_id)).execute(&mut *tx).await.map_err(db)?;
        if advance {
            sqlx::query("UPDATE subject_reflection_progress SET last_change_id=GREATEST(last_change_id,$2) WHERE subject_id=$1").bind(selected.subject_id.0).bind(through_change).execute(&mut *tx).await.map_err(db)?;
        }
        if advance {
            sqlx::query("UPDATE maintenance_needs SET state='pending',last_problem_code=NULL WHERE subject_id=$1 AND need_id=$2 AND trigger_revision=$3 AND state='blocked'").bind(selected.subject_id.0).bind(selected.need_id).bind(selected.trigger_revision as i64).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("UPDATE maintenance_needs SET trigger_revision=trigger_revision+1,state='pending',last_problem_code=NULL WHERE subject_id=$1 AND need_id=$2 AND trigger_revision=$3 AND (EXISTS(SELECT 1 FROM experience_items WHERE subject_id=$1 AND recorded_seq>$4 AND recorded_seq<=maintenance_needs.trigger_authority_seq AND source_class NOT IN ('generated','generation','simulation')) OR EXISTS(SELECT 1 FROM subject_reflection_source_changes c JOIN subject_reflection_progress p USING(subject_id) WHERE c.subject_id=$1 AND c.change_id>p.last_change_id))").bind(selected.subject_id.0).bind(selected.need_id).bind(selected.trigger_revision as i64).bind(through).execute(&mut *tx).await.map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }
}

impl CognitiveRuntimeService {
    /// Actual owner lifecycle changes are metadata-only review inputs, never
    /// invented external evidence or a reason to replay hidden source bodies.
    pub async fn reflection_source_changed_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        subject: SubjectId,
        references: &[CognitiveRef],
        sequence: i64,
        reason: &str,
    ) -> Result<()> {
        if references.len() > 4096 {
            return Err(Error::Invalid(
                "reflection source change exceeds batch bound".into(),
            ));
        }
        if self.reflection_domains(subject).await?.is_empty() {
            return Ok(());
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("subject-reflection:{}", subject.0))
            .execute(&mut **tx)
            .await
            .map_err(db)?;
        let mut changed = false;
        for reference in references {
            changed|=sqlx::query("INSERT INTO subject_reflection_source_changes(subject_id,reference,reason,authority_sequence,recorded_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING").bind(subject.0).bind(serde_json::to_value(reference).map_err(|e|Error::Infrastructure(e.to_string()))?).bind(reason).bind(sequence).bind(self.now(subject)).execute(&mut **tx).await.map_err(db)?.rows_affected()>0;
        }
        if changed {
            self.trigger_reflection_in(tx, subject, sequence).await?;
        }
        Ok(())
    }
}
