//! Expiring Runtime query correlation, separate from Cognition Authority.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_configuration::*;
use nous_persistence::database_error as db;
pub const QUERY_FEEDBACK_RETENTION: ConfigKey<u64> =
    ConfigKey::new("runtime.query_feedback_retention");
pub(crate) fn register(registry: &mut ConfigRegistryBuilder) -> Result<()> {
    registry.register(
        QUERY_FEEDBACK_RETENTION,
        "cognitive-runtime",
        "Query feedback retention in seconds, measured by the Subject CognitiveClock.",
        604800,
        ConfigExposure::Developer,
        ConfigScopePolicy::SubjectOverrideAllowed,
        ConfigApplyMode::Live,
        ConfigSemanticEffect::Operational,
        |seconds| {
            if (1..=31536000).contains(seconds) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "query feedback retention must be 1..31536000 seconds".into(),
                ))
            }
        },
    )
}
impl CognitiveRuntimeService {
    pub async fn record_query_feedback(
        &self,
        bound: &BoundQuery,
        result: &CognitiveQueryResult,
    ) -> Result<()> {
        if result.query_id != bound.query_id {
            return Err(Error::Invalid(
                "query feedback result does not match its bound query".into(),
            ));
        }
        let subject = bound.source_query.subject;
        let created = self.now(subject);
        let retention = bound.config_snapshot.get(QUERY_FEEDBACK_RETENTION)?;
        let expires = created
            .checked_add_signed(chrono::Duration::seconds(
                i64::try_from(retention)
                    .map_err(|_| Error::Invalid("query retention overflow".into()))?,
            ))
            .ok_or_else(|| Error::Invalid("query expiry overflow".into()))?;
        let revisions = result
            .results
            .iter()
            .filter_map(|hit| {
                let reference = hit.revision.as_ref().unwrap_or(&hit.reference);
                matches!(
                    reference,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                        | CognitiveRef::JournalRevision(_)
                )
                .then(|| reference.clone())
            })
            .collect::<Vec<_>>();
        if revisions.len() > 256 {
            return Err(Error::Invalid(
                "query feedback returned revisions exceed bound".into(),
            ));
        }
        let activation = &bound.activation;
        let signals = serde_json::json!({
            "explicit_tags":activation.explicit_tags.iter().take(128).collect::<Vec<_>>(),
            "inferred_tags":activation.inferred_tags.iter().take(8).collect::<Vec<_>>(),
            "novel_concepts":activation.novel_concepts.iter().take(4).collect::<Vec<_>>(),
            "associative_seeds":activation.seeds.iter().take(128).collect::<Vec<_>>(),
            "profile_id":bound.retrieval_policy.cognitive_profile.id(),
            "concept_generation":activation.concept_generation,
        });
        let prepared_digest =
            canonical_request_digest("prepared_query_feedback", subject, &bound.source_query)?;
        let activation_digest =
            canonical_request_digest("query_activation_feedback", subject, &bound.activation)?;
        let mut tx = self.store.begin().await?;
        sqlx::query("DELETE FROM query_feedback_records WHERE subject_id=$1 AND expires_at<=$2")
            .bind(subject.0)
            .bind(created)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let stored=sqlx::query("INSERT INTO query_feedback_records(subject_id,query_id,session_id,work_context_id,prepared_query_digest,query_activation_digest,signals,returned_revision_refs,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(subject_id,query_id) DO UPDATE SET returned_revision_refs=EXCLUDED.returned_revision_refs WHERE query_feedback_records.prepared_query_digest=EXCLUDED.prepared_query_digest AND query_feedback_records.query_activation_digest=EXCLUDED.query_activation_digest")
            .bind(subject.0).bind(bound.query_id).bind(bound.source_query.session.map(|session|session.0)).bind(bound.source_query.work_context).bind(prepared_digest).bind(activation_digest).bind(signals).bind(serde_json::json!(revisions)).bind(created).bind(expires).execute(&mut *tx).await.map_err(db)?;
        if stored.rows_affected() != 1 {
            return Err(DomainError::new(
                DomainErrorCode::OperationIdConflict,
                "Query feedback identity has different prepared activation",
            )
            .into());
        }
        tx.commit().await.map_err(db)
    }
}
pub(crate) async fn validate_query_use_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    query_id: uuid::Uuid,
    reference: &CognitiveRef,
    now: DateTime<Utc>,
) -> Result<()> {
    let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM query_feedback_records WHERE subject_id=$1 AND query_id=$2 AND expires_at>$3 AND returned_revision_refs @> $4::jsonb)")
        .bind(subject.0).bind(query_id).bind(now).bind(serde_json::json!([reference])).fetch_one(&mut **tx).await.map_err(db)?;
    if !found {
        return Err(Error::Invalid(
            "query feedback expired, belongs to another Subject, or did not return this revision"
                .into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryFeedbackSignals {
    pub explicit_tags: Vec<TagActivation>,
    pub inferred_tags: Vec<TagActivation>,
    pub novel_concepts: Vec<NovelConceptHypothesis>,
    pub associative_seeds: Vec<ActivationSeed>,
    pub profile_id: String,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct LinkedQueryFeedback {
    pub query_id: uuid::Uuid,
    pub used_revision: CognitiveRef,
    pub use_kind: UseKind,
    pub signals: QueryFeedbackSignals,
}
/// The Memory planner consumes bounded Runtime signals through the Runtime owner contract.
pub async fn linked_query_feedback(
    store: &AuthorityStore,
    subject: SubjectId,
    focus: &CognitiveRef,
    now: DateTime<Utc>,
) -> Result<Vec<LinkedQueryFeedback>> {
    use sqlx::Row;
    let (kind, value) = reference_parts(focus);
    let rows=sqlx::query("SELECT * FROM (SELECT DISTINCT ON (q.query_id) q.query_id,q.signals,e.use_kind,e.recorded_at FROM cognitive_use_events e JOIN query_feedback_records q ON q.subject_id=e.subject_id AND q.query_id=e.query_id WHERE e.subject_id=$1 AND e.ref_kind=$2 AND e.ref_value=$3 AND e.use_kind<>'presented' AND q.expires_at>$4 ORDER BY q.query_id,e.recorded_at DESC,e.event_id) recent ORDER BY recorded_at DESC,query_id LIMIT 8")
        .bind(subject.0).bind(kind).bind(value).bind(now).fetch_all(store.pool()).await.map_err(db)?;
    rows.into_iter()
        .map(|row| {
            let mut signals: QueryFeedbackSignals =
                serde_json::from_value(row.try_get("signals").map_err(db)?)
                    .map_err(|error| Error::Infrastructure(error.to_string()))?;
            signals.explicit_tags.truncate(16);
            signals.inferred_tags.truncate(8);
            signals.novel_concepts.truncate(4);
            signals.associative_seeds.truncate(16);
            let kind: String = row.try_get("use_kind").map_err(db)?;
            Ok(LinkedQueryFeedback {
                query_id: row.try_get("query_id").map_err(db)?,
                used_revision: focus.clone(),
                use_kind: serde_json::from_value(serde_json::json!(kind))
                    .map_err(|error| Error::Infrastructure(error.to_string()))?,
                signals,
            })
        })
        .collect()
}
