// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use sqlx::{Postgres, Transaction};
use std::collections::BTreeSet;

struct JournalRevisionWrite {
    journal: JournalId,
    revision: JournalRevisionId,
    parent: Option<JournalRevisionId>,
    revision_no: i32,
    source_extent: TemporalExtent,
    formed: DateTime<Utc>,
    recorded: DateTime<Utc>,
    producer: Option<Uuid>,
}

impl MemoryService {
    pub async fn commit_journal(&self, input: JournalInput) -> Result<JournalView> {
        validate_journal(&input)?;
        self.store.require_subject(input.subject).await?;
        let started = self.cognition.now(input.subject);
        let digest = operation_digest("journal_commit", input.subject, &input)?;
        let mut mutation = match self
            .start_mutation(input.subject, input.operation_id, "journal_commit", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::Unavailable("Journal commit is in progress".into()));
                }
                let revision = JournalRevisionId(
                    receipt
                        .result_revision
                        .ok_or_else(|| Error::NotFound("Journal result was purged".into()))?,
                );
                return self.journal_revision(input.subject, revision).await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let source_extent = validate_sources_in(mutation.tx(), &input).await?;
        fence_source_snapshot_in(mutation.tx(), &input).await?;
        let (journal, parent, revision_no) = journal_target_in(mutation.tx(), &input).await?;
        let basis: Vec<_> = input
            .points
            .iter()
            .flat_map(|point| point.basis.clone())
            .collect();
        self.validate_basis_in_tx(mutation.tx(), input.subject, &basis)
            .await?;
        if let Some(target) = &input.target {
            self.validate_object_dependency_cycle(
                input.subject,
                &format!("journal:{}", target.journal_id.0),
                &basis,
            )
            .await?;
        }
        let formed = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started)
            .await?;
        let recorded = self.cognition.now(input.subject);
        let producer = journal_producer_in(mutation.tx(), input.producer.as_ref()).await?;
        let revision = JournalRevisionId::new();
        self.write_journal_revision_in(
            mutation.tx(),
            &input,
            JournalRevisionWrite {
                journal,
                revision,
                parent,
                revision_no,
                source_extent,
                formed,
                recorded,
                producer,
            },
        )
        .await?;
        let sequence = mutation.invalidate(ProjectionInvalidation::text()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::JournalRevision(revision),
            sequence,
        )
        .await?;
        if parent.is_some() {
            self.invalidate_object_dependents_in(
                mutation.tx(),
                input.subject,
                "journal",
                &[journal.0],
                sequence,
                "source_revised",
            )
            .await?;
        }
        self.wake_journal_revalidation_in(mutation.tx(), input.subject, journal, sequence)
            .await?;
        self.schedule_journal_consolidation_in(
            mutation.tx(),
            input.subject,
            revision,
            sequence,
            recorded,
        )
        .await?;

        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::Journal(journal),
                    CognitiveRef::JournalRevision(revision),
                ],
            )
            .await?;
        mutation
            .commit(
                "journal",
                Some(&journal.0.to_string()),
                Some(revision.0),
                None,
            )
            .await?;
        self.journal(input.subject, journal, Some(revision)).await
    }
    async fn write_journal_revision_in(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        input: &JournalInput,
        write: JournalRevisionWrite,
    ) -> Result<()> {
        let JournalRevisionWrite {
            journal,
            revision,
            parent,
            revision_no,
            source_extent,
            formed,
            recorded,
            producer,
        } = write;
        if parent.is_none() {
            sqlx::query("INSERT INTO journal_objects(journal_id,subject_id,current_revision_id,acceptance_state,integrity_state,suppression_state,purge_state,created_at) VALUES($1,$2,$3,'accepted','valid','normal','normal',$4)")
                .bind(journal.0).bind(input.subject.0).bind(revision.0).bind(recorded).execute(&mut **tx).await.map_err(db)?;
        }
        let (kind, start, end) = temporal_columns(&source_extent);
        sqlx::query("INSERT INTO journal_revisions(journal_revision_id,journal_id,subject_id,revision_no,parent_revision_id,revision_intent,title,temporal_scope_kind,temporal_scope_start,temporal_scope_end,narrative,formed_at,recorded_at,producer_signature_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
            .bind(revision.0).bind(journal.0).bind(input.subject.0).bind(revision_no).bind(parent.map(|id|id.0)).bind(input.target.as_ref().map(|target|&target.intent)).bind(&input.title).bind(kind).bind(start).bind(end).bind(&input.narrative).bind(formed).bind(recorded).bind(producer).execute(&mut **tx).await.map_err(db)?;
        insert_journal_points(tx, revision, &input.points).await?;
        self.store
            .ensure_identity_addresses_in(
                tx,
                input.subject,
                &[
                    CognitiveRef::Journal(journal),
                    CognitiveRef::JournalRevision(revision),
                ],
                input.title.as_deref().unwrap_or(""),
            )
            .await?;
        for source in &input.sources {
            sqlx::query("INSERT INTO journal_revision_sources(journal_revision_id,ref_kind,ref_value,source_epoch) VALUES($1,'episode_revision',$2,$3)")
                .bind(revision.0).bind(source.revision.0.to_string()).bind(source.expected_epoch).execute(&mut **tx).await.map_err(db)?;
        }
        sqlx::query("UPDATE journal_objects SET current_revision_id=$2,object_epoch=object_epoch+CASE WHEN $3 THEN 1 ELSE 0 END,integrity_state='valid' WHERE journal_id=$1")
            .bind(journal.0).bind(revision.0).bind(parent.is_some()).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }
    async fn schedule_journal_consolidation_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        revision: JournalRevisionId,
        sequence: i64,
        recorded: DateTime<Utc>,
    ) -> Result<()> {
        self.cognition
            .enqueue_maintenance_in(
                tx,
                &nous_runtime::MaintenanceRequest {
                    subject,
                    kind: "memory_consolidate".into(),
                    scope_kind: "journal_revision".into(),
                    scope_ref: revision.0.to_string(),
                    trigger_authority_seq: sequence,
                    due_at: recorded
                        + chrono::Duration::seconds(
                            self.configuration
                                .snapshot_for_subject(subject)?
                                .get(super::super::longitudinal_policy::CONSOLIDATION_DELAY)?
                                as i64,
                        ),
                    priority: 40,
                },
            )
            .await?;
        Ok(())
    }
}

fn validate_journal(input: &JournalInput) -> Result<()> {
    if input.sources.is_empty()
        || input.sources.len() > 64
        || input.points.is_empty()
        || input.points.len() > 128
        || input.title.as_ref().is_some_and(|text| text.len() > 8192)
        || input.narrative.trim().is_empty()
        || input.narrative.len() > 131072
        || input
            .sources
            .iter()
            .map(|source| source.revision.0)
            .collect::<BTreeSet<_>>()
            .len()
            != input.sources.len()
        || input.target.as_ref().is_some_and(|target| {
            !["revalidate", "reinterpret", "reframe"].contains(&target.intent.as_str())
        })
    {
        return Err(Error::Invalid(
            "Journal scope or content is out of bounds".into(),
        ));
    }
    for point in &input.points {
        if point.text.trim().is_empty() || point.text.len() > 8192 || point.basis.len() > 16 {
            return Err(Error::Invalid("Journal point is out of bounds".into()));
        }
        validate_exact_basis(&point.basis)?;
    }
    Ok(())
}

async fn fence_source_snapshot_in(
    tx: &mut Transaction<'_, Postgres>,
    input: &JournalInput,
) -> Result<()> {
    let watermark: i64 =
        sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1 FOR UPDATE")
            .bind(input.subject.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(db)?;
    if watermark != input.expected_authority_seq {
        return Err(nous_core::DomainError::new(
            nous_core::DomainErrorCode::StaleRevision,
            "Journal input snapshot is stale",
        )
        .with_context("expected_authority_seq", input.expected_authority_seq)
        .with_context("actual_authority_seq", watermark)
        .into());
    }
    Ok(())
}

async fn journal_target_in(
    tx: &mut Transaction<'_, Postgres>,
    input: &JournalInput,
) -> Result<(JournalId, Option<JournalRevisionId>, i32)> {
    let Some(target) = &input.target else {
        return Ok((JournalId::new(), None, 1));
    };
    let row = sqlx::query("SELECT o.current_revision_id,o.object_epoch,o.purge_state,r.revision_no FROM journal_objects o JOIN journal_revisions r ON r.journal_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.journal_id=$2 FOR UPDATE OF o")
        .bind(input.subject.0).bind(target.journal_id.0).fetch_optional(&mut **tx).await.map_err(db)?
        .ok_or_else(|| nous_core::DomainError::new(nous_core::DomainErrorCode::StaleRevision, "Journal target disappeared"))?;
    if row.get::<Uuid, _>("current_revision_id") != target.expected_revision.0
        || row.get::<i64, _>("object_epoch") != target.expected_epoch
        || row.get::<String, _>("purge_state") != "normal"
    {
        return Err(nous_core::DomainError::new(
            nous_core::DomainErrorCode::StaleRevision,
            "Journal target revision or lifecycle is stale",
        )
        .into());
    }
    validate_journal_scope_in(tx, input, target).await?;
    Ok((
        target.journal_id,
        Some(target.expected_revision),
        row.get::<i32, _>("revision_no") + 1,
    ))
}

async fn validate_journal_scope_in(
    tx: &mut Transaction<'_, Postgres>,
    input: &JournalInput,
    target: &JournalTarget,
) -> Result<()> {
    let current: Vec<Uuid> = input
        .sources
        .iter()
        .map(|source| source.revision.0)
        .collect();
    let connected: i64 = sqlx::query_scalar("WITH RECURSIVE old_scope AS (SELECT r.episode_revision_id,r.episode_id FROM journal_revision_sources s JOIN episode_revisions r ON r.episode_revision_id::text=s.ref_value WHERE s.journal_revision_id=$1 AND s.ref_kind='episode_revision'), ancestry(origin,revision) AS (SELECT id,id FROM unnest($2::uuid[]) id UNION SELECT a.origin,e.ancestor FROM ancestry a JOIN LATERAL (SELECT parent_revision_id AS ancestor FROM episode_revisions WHERE episode_revision_id=a.revision AND parent_revision_id IS NOT NULL UNION SELECT to_revision_id FROM episode_revision_relations WHERE from_revision_id=a.revision AND relation IN ('derived_from','split_from','merged_from')) e ON true) SELECT COUNT(DISTINCT a.origin) FROM ancestry a JOIN episode_revisions r ON r.episode_revision_id=a.revision JOIN old_scope o ON o.episode_revision_id=a.revision OR o.episode_id=r.episode_id")
        .bind(target.expected_revision.0).bind(&current).fetch_one(&mut **tx).await.map_err(db)?;
    if connected != current.len() as i64 {
        return Err(Error::Invalid(
            "Journal revision sources describe a different narrative scope".into(),
        ));
    }
    Ok(())
}

async fn validate_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    input: &JournalInput,
) -> Result<TemporalExtent> {
    let ids: Vec<Uuid> = input
        .sources
        .iter()
        .map(|source| source.revision.0)
        .collect();
    let rows = sqlx::query("SELECT o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,r.episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM episode_objects o JOIN episode_revisions r ON r.episode_id=o.episode_id WHERE o.subject_id=$1 AND r.episode_revision_id=ANY($2::uuid[]) ORDER BY o.episode_id FOR SHARE OF o")
        .bind(input.subject.0).bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
    if rows.len() != ids.len() {
        return Err(nous_core::DomainError::new(
            nous_core::DomainErrorCode::StaleRevision,
            "Journal source disappeared",
        )
        .into());
    }
    validate_source_catalog_in(tx, input, &ids).await?;
    let mut extents = Vec::with_capacity(rows.len());
    for row in rows {
        let revision: Uuid = row.get("episode_revision_id");
        let expected = input
            .sources
            .iter()
            .find(|source| source.revision.0 == revision)
            .ok_or_else(|| Error::Internal("unbound Journal source".into()))?;
        if row.get::<Uuid, _>("current_revision_id") != revision
            || row.get::<i64, _>("object_epoch") != expected.expected_epoch
            || row.get::<String, _>("acceptance_state") != "accepted"
            || row.get::<String, _>("integrity_state") != "valid"
            || row.get::<String, _>("suppression_state") != "normal"
            || row.get::<String, _>("purge_state") != "normal"
        {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Journal source state is stale",
            )
            .into());
        }
        extents.push(temporal_from_columns(
            row.try_get("experience_time_kind").map_err(db)?,
            row.try_get("experience_time_start").map_err(db)?,
            row.try_get("experience_time_end").map_err(db)?,
        )?);
    }
    Ok(aggregate_extent(&extents))
}

async fn validate_source_catalog_in(
    tx: &mut Transaction<'_, Postgres>,
    input: &JournalInput,
    ids: &[Uuid],
) -> Result<()> {
    let mut allowed = ids
        .iter()
        .map(|id| format!("episode_revision:{id}"))
        .collect::<BTreeSet<_>>();
    let members = sqlx::query("SELECT ref_kind,ref_value FROM episode_revision_members WHERE episode_revision_id=ANY($1::uuid[])")
        .bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
    for member in members {
        allowed.insert(format!(
            "{}:{}",
            member.get::<String, _>("ref_kind"),
            member.get::<String, _>("ref_value")
        ));
    }
    let rows_basis = sqlx::query("SELECT occurrence_id,source_region_id,derived_representation_id,derived_region_id FROM episode_revision_basis WHERE basis_kind='evidence' AND episode_revision_id=ANY($1::uuid[])")
        .bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
    let mut evidence_catalog = BTreeSet::new();
    for row in rows_basis {
        evidence_catalog.insert((
            row.get::<Uuid, _>("occurrence_id"),
            row.get::<Option<Uuid>, _>("source_region_id"),
            row.get::<Option<Uuid>, _>("derived_representation_id"),
            row.get::<Option<Uuid>, _>("derived_region_id"),
        ));
    }
    let dependencies = sqlx::query("SELECT basis_kind AS target_ref_kind,basis_ref AS target_ref FROM episode_revision_basis WHERE basis_kind<>'evidence' AND episode_revision_id=ANY($1::uuid[])")
        .bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
    for row in dependencies {
        allowed.insert(format!(
            "{}:{}",
            row.get::<String, _>("target_ref_kind"),
            row.get::<String, _>("target_ref")
        ));
    }
    for point in &input.points {
        for basis in &point.basis {
            let key = match basis {
                RevisionBasis::Evidence(evidence) => {
                    let (source, representation, region) = match evidence.locator {
                        EvidenceLocator::WholeOccurrence => (None, None, None),
                        EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                        EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                        EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
                    };
                    if evidence_catalog.contains(&(
                        evidence.occurrence_id.0,
                        source,
                        representation,
                        region,
                    )) {
                        continue;
                    }
                    if !matches!(evidence.locator, EvidenceLocator::WholeOccurrence) {
                        return Err(Error::Invalid(
                            "Journal evidence locator is outside the source catalog".into(),
                        ));
                    }
                    CognitiveRef::Occurrence(evidence.occurrence_id).to_string()
                }
                RevisionBasis::CognitionDependency(dependency) => {
                    dependency.target_revision.to_string()
                }
                _ => {
                    return Err(Error::Invalid(
                        "Journal point support is outside the source catalog".into(),
                    ));
                }
            };
            if !allowed.contains(&key) {
                return Err(Error::Invalid(
                    "Journal point support is outside the source catalog".into(),
                ));
            }
        }
    }
    Ok(())
}

fn aggregate_extent(extents: &[TemporalExtent]) -> TemporalExtent {
    if extents
        .iter()
        .any(|extent| matches!(extent, TemporalExtent::Unknown))
    {
        return TemporalExtent::Unknown;
    }
    let starts: Vec<Option<DateTime<Utc>>> = extents
        .iter()
        .map(|extent| match extent {
            TemporalExtent::Instant { at } => Some(*at),
            TemporalExtent::Interval { start, .. } => *start,
            TemporalExtent::Unknown => None,
        })
        .collect();
    let ends: Vec<Option<DateTime<Utc>>> = extents
        .iter()
        .map(|extent| match extent {
            TemporalExtent::Instant { at } => Some(*at),
            TemporalExtent::Interval { end, .. } => *end,
            TemporalExtent::Unknown => None,
        })
        .collect();
    let start = starts
        .iter()
        .all(Option::is_some)
        .then(|| starts.into_iter().flatten().min())
        .flatten();
    let end = ends
        .iter()
        .all(Option::is_some)
        .then(|| ends.into_iter().flatten().max())
        .flatten();
    if let (Some(start), Some(end)) = (start, end)
        && start == end
    {
        TemporalExtent::Instant { at: start }
    } else {
        TemporalExtent::Interval { start, end }
    }
}

async fn insert_journal_points(
    tx: &mut Transaction<'_, Postgres>,
    revision: JournalRevisionId,
    points: &[JournalPoint],
) -> Result<()> {
    for (ordinal, point) in points.iter().enumerate() {
        sqlx::query("INSERT INTO journal_revision_points(journal_revision_id,ordinal,role,text) VALUES($1,$2,$3,$4)")
            .bind(revision.0).bind(ordinal as i32).bind(point.role.as_str()).bind(&point.text).execute(&mut **tx).await.map_err(db)?;
        for (number, basis) in point.basis.iter().enumerate() {
            sqlx::query("INSERT INTO journal_point_basis(journal_revision_id,ordinal,basis_no,basis) VALUES($1,$2,$3,$4)")
                .bind(revision.0).bind(ordinal as i32).bind(number as i32).bind(serde_json::to_value(basis).map_err(|error|Error::Invalid(error.to_string()))?).execute(&mut **tx).await.map_err(db)?;
        }
    }
    Ok(())
}

async fn journal_producer_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    producer: Option<&nous_core::ProducerSignature>,
) -> Result<Option<Uuid>> {
    let Some(signature) = producer else {
        return Ok(None);
    };
    if signature.operation != CapabilityOperation::JournalSynthesisText {
        return Err(Error::Invalid(
            "Journal requires a synthesis producer".into(),
        ));
    }
    AuthorityStore::register_producer_in(tx, signature)
        .await
        .map(Some)
}
