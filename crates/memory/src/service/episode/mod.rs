// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
mod mutation;
mod read;

use chrono::{DateTime, Utc};
use nous_core::{CognitiveRef, EpisodeId, EpisodeRevisionId, OperationId, SubjectId};
use nous_persistence::database_error as db;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::collections::BTreeSet;
use uuid::Uuid;
mod partition;
pub use partition::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeMemberInput {
    pub reference: CognitiveRef,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeContent {
    pub title: Option<String>,
    pub parent_episode_revision_id: Option<EpisodeRevisionId>,
    pub experience_time: TemporalExtent,
    pub boundary_explanation: String,
    pub producer_signature_id: Option<Uuid>,
    pub members: Vec<EpisodeMemberInput>,
    pub basis: Vec<RevisionBasis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub track_key: String,
    pub content: EpisodeContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseEpisodeInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub episode_id: EpisodeId,
    pub expected_object_epoch: i64,
    pub intent: String,
    pub content: EpisodeContent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeObject {
    pub episode_id: EpisodeId,
    pub subject_id: SubjectId,
    pub track_key: String,
    pub current_revision_id: EpisodeRevisionId,
    pub object_epoch: i64,
    pub acceptance_state: AcceptanceState,
    pub integrity_state: IntegrityState,
    pub suppression_state: SuppressionState,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRevision {
    pub episode_revision_id: EpisodeRevisionId,
    pub episode_id: EpisodeId,
    pub subject_id: SubjectId,
    pub revision_no: i32,
    pub parent_revision_id: Option<EpisodeRevisionId>,
    pub revision_intent: Option<String>,
    pub title: Option<String>,
    pub parent_episode_revision_id: Option<EpisodeRevisionId>,
    pub experience_time: TemporalExtent,
    pub boundary_explanation: String,
    pub formed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer_signature_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeMember {
    pub ordinal: i32,
    pub reference: CognitiveRef,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRelation {
    pub from_revision_id: EpisodeRevisionId,
    pub to_revision_id: EpisodeRevisionId,
    pub relation: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeView {
    pub object: EpisodeObject,
    pub revision: EpisodeRevision,
    pub members: Vec<EpisodeMember>,
    pub basis: Vec<RevisionBasis>,
    pub relations: Vec<EpisodeRelation>,
}

fn validate_episode_track(track: &str) -> Result<()> {
    if !track.is_empty() && (track.len() > 128 || track.chars().any(char::is_control)) {
        return Err(Error::Invalid("Episode track_key is out of bounds".into()));
    }
    Ok(())
}

impl EpisodeContent {
    pub fn validate(&self) -> Result<()> {
        if self.title.as_ref().is_some_and(|value| value.len() > 8192) {
            return Err(Error::Invalid("Episode title is out of bounds".into()));
        }
        self.experience_time.validate()?;
        if self.boundary_explanation.trim().is_empty() || self.boundary_explanation.len() > 16384 {
            return Err(Error::Invalid(
                "Episode boundary explanation is out of bounds".into(),
            ));
        }
        if self.members.is_empty() || self.members.len() > 2048 {
            return Err(Error::Invalid("Episode members are out of bounds".into()));
        }
        if self.basis.is_empty() || self.basis.len() > 256 {
            return Err(Error::Invalid("Episode basis are out of bounds".into()));
        }
        let mut keys = BTreeSet::new();
        for member in &self.members {
            if member.role.is_empty()
                || member.role.len() > 128
                || !keys.insert(member.reference.to_string())
            {
                return Err(Error::Invalid(
                    "Episode members must be unique and bounded".into(),
                ));
            }
        }
        if self.basis.iter().any(|basis| matches!(basis, RevisionBasis::CognitionDependency(value) if !matches!(value.target_revision, CognitiveRef::MemoryRevision(_) | CognitiveRef::CognitiveSchemaRevision(_) | CognitiveRef::EpisodeRevision(_)))) {
            return Err(Error::Invalid("Episode support must target a Memory, Schema, or Episode revision".into()));
        }
        validate_exact_basis(&self.basis)
    }
}

async fn validate_episode_refs_in_tx(
    store: &AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    members: &[EpisodeMemberInput],
) -> Result<()> {
    for member in members {
        if !matches!(
            member.reference,
            CognitiveRef::Occurrence(_)
                | CognitiveRef::MemoryRevision(_)
                | CognitiveRef::CognitiveSchemaRevision(_)
                | CognitiveRef::EpisodeRevision(_)
        ) {
            return Err(Error::Invalid("invalid Episode member reference".into()));
        }
        if !store
            .reference_in_subject_tx(tx, subject, &member.reference)
            .await?
        {
            return Err(Error::FailedPrecondition(
                "Episode member is outside Subject".into(),
            ));
        }
    }
    Ok(())
}

async fn validate_parent_and_overlap(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    track: &str,
    parent: Option<EpisodeRevisionId>,
    time: &TemporalExtent,
    exclude: Option<Uuid>,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "episode-segment:{}:{}:{:?}",
            subject.0,
            track,
            parent.map(|value| value.0)
        ))
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    if let Some(parent) = parent {
        let parent_time = sqlx::query("SELECT experience_time_kind,experience_time_start,experience_time_end FROM episode_revisions WHERE episode_revision_id=$1 AND subject_id=$2").bind(parent.0).bind(subject.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(|| Error::Invalid("Episode parent is outside Subject".into()))?;
        let parent_extent = temporal_from_columns(
            parent_time.try_get("experience_time_kind").map_err(db)?,
            parent_time.try_get("experience_time_start").map_err(db)?,
            parent_time.try_get("experience_time_end").map_err(db)?,
        )?;
        if !contains_extent(&parent_extent, time) {
            return Err(Error::Invalid(
                "Episode child lies outside parent experience span".into(),
            ));
        }
    }
    let rows = sqlx::query("SELECT r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM episode_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.track_key=$2 AND o.acceptance_state='accepted' AND r.parent_episode_revision_id IS NOT DISTINCT FROM $3 AND ($4::uuid IS NULL OR o.episode_id<>$4)").bind(subject.0).bind(track).bind(parent.map(|value| value.0)).bind(exclude).fetch_all(&mut **tx).await.map_err(db)?;
    for row in rows {
        let sibling = temporal_from_columns(
            row.try_get("experience_time_kind").map_err(db)?,
            row.try_get("experience_time_start").map_err(db)?,
            row.try_get("experience_time_end").map_err(db)?,
        )?;
        if known_overlap(&sibling, time) {
            return Err(Error::Conflict(
                "same-track Episode sibling experience spans overlap".into(),
            ));
        }
    }
    Ok(())
}

pub(in crate::service) async fn validate_episode_reaccept_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    episode: EpisodeId,
) -> Result<()> {
    let row = sqlx::query("SELECT o.track_key,r.parent_episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM episode_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.episode_id=$2")
        .bind(subject.0).bind(episode.0).fetch_one(&mut **tx).await.map_err(db)?;
    let time = temporal_from_columns(
        row.try_get("experience_time_kind").map_err(db)?,
        row.try_get("experience_time_start").map_err(db)?,
        row.try_get("experience_time_end").map_err(db)?,
    )?;
    validate_parent_and_overlap(
        tx,
        subject,
        &row.try_get::<String, _>("track_key").map_err(db)?,
        row.try_get::<Option<Uuid>, _>("parent_episode_revision_id")
            .map_err(db)?
            .map(EpisodeRevisionId),
        &time,
        Some(episode.0),
    )
    .await
}

async fn ensure_no_parent_cycle(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    episode: Uuid,
    mut parent: Uuid,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    loop {
        if parent == episode || !seen.insert(parent) {
            return Err(Error::Conflict(
                "Episode parent hierarchy contains a cycle".into(),
            ));
        }
        let row = sqlx::query(
            "SELECT episode_id,parent_episode_revision_id FROM episode_revisions WHERE episode_revision_id=$1",
        )
        .bind(parent)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::Invalid("Episode parent revision does not exist".into()))?;
        let parent_episode: Uuid = row.try_get("episode_id").map_err(db)?;
        if parent_episode == episode {
            return Err(Error::Conflict(
                "Episode parent hierarchy contains a cycle".into(),
            ));
        }
        let next: Option<Uuid> = row.try_get("parent_episode_revision_id").map_err(db)?;
        let Some(next) = next else {
            return Ok(());
        };
        parent = next;
    }
}

struct EpisodeRevisionWrite {
    episode: EpisodeId,
    revision: EpisodeRevisionId,
    parent: Option<EpisodeRevisionId>,
    intent: Option<String>,
    number: i32,
    formed_at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
}

async fn insert_episode_revision(
    store: &AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    content: &EpisodeContent,
    write: EpisodeRevisionWrite,
) -> Result<()> {
    let EpisodeRevisionWrite {
        episode,
        revision,
        parent: parent_revision,
        intent,
        number: revision_no,
        formed_at,
        recorded_at,
    } = write;
    let (kind, start, end) = temporal_columns(&content.experience_time);
    sqlx::query("INSERT INTO episode_revisions(episode_revision_id,episode_id,subject_id,revision_no,parent_revision_id,revision_intent,title,parent_episode_revision_id,experience_time_kind,experience_time_start,experience_time_end,boundary_explanation,formed_at,recorded_at,producer_signature_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)")
        .bind(revision.0).bind(episode.0).bind(subject.0).bind(revision_no).bind(parent_revision.map(|value| value.0)).bind(intent).bind(&content.title).bind(content.parent_episode_revision_id.map(|value| value.0)).bind(kind).bind(start).bind(end).bind(&content.boundary_explanation).bind(formed_at).bind(recorded_at).bind(content.producer_signature_id).execute(&mut **tx).await.map_err(db)?;
    for (ordinal, member) in content.members.iter().enumerate() {
        let (kind, value) = reference_parts(&member.reference);
        sqlx::query("INSERT INTO episode_revision_members(episode_revision_id,ordinal,ref_kind,ref_value,role) VALUES($1,$2,$3,$4,$5)").bind(revision.0).bind(ordinal as i32).bind(kind).bind(value).bind(&member.role).execute(&mut **tx).await.map_err(db)?;
    }
    for (basis_no, basis) in content.basis.iter().enumerate() {
        insert_episode_support(tx, revision, basis_no as i32, basis).await?;
    }
    store
        .ensure_identity_addresses_in(
            tx,
            subject,
            &[
                CognitiveRef::Episode(episode),
                CognitiveRef::EpisodeRevision(revision),
            ],
            content.title.as_deref().unwrap_or(""),
        )
        .await?;
    Ok(())
}

async fn insert_episode_support(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    revision: EpisodeRevisionId,
    basis_no: i32,
    basis: &RevisionBasis,
) -> Result<()> {
    let (kind, reference, occurrence, source_region, derived, derived_region, role) = match basis {
        RevisionBasis::Evidence(value) => {
            let (source, derived, derived_region) = match value.locator {
                EvidenceLocator::WholeOccurrence => (None, None, None),
                EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
            };
            (
                "evidence".to_owned(),
                value.occurrence_id.0.to_string(),
                Some(value.occurrence_id.0),
                source,
                derived,
                derived_region,
                value.basis_role.as_str().to_owned(),
            )
        }
        RevisionBasis::CognitionDependency(value) => {
            let (kind, reference) = reference_parts(&value.target_revision);
            (
                kind,
                reference,
                None,
                None,
                None,
                None,
                value.basis_role.as_str().to_owned(),
            )
        }
        RevisionBasis::Seed(_) => {
            return Err(Error::Invalid(
                "Episode cannot use Cognitive Seed support".into(),
            ));
        }
    };
    sqlx::query("INSERT INTO episode_revision_basis(episode_revision_id,basis_no,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(revision.0).bind(basis_no).bind(kind).bind(reference).bind(role).bind(occurrence).bind(source_region).bind(derived).bind(derived_region).bind(epistemic_relation_text(basis.epistemic_relation())).execute(&mut **tx).await.map_err(db)?;
    Ok(())
}

fn known_overlap(left: &TemporalExtent, right: &TemporalExtent) -> bool {
    match (left, right) {
        (TemporalExtent::Instant { at: left }, TemporalExtent::Instant { at: right }) => {
            left == right
        }
        (TemporalExtent::Unknown, _) | (_, TemporalExtent::Unknown) => false,
        (TemporalExtent::Instant { at }, TemporalExtent::Interval { start, end })
        | (TemporalExtent::Interval { start, end }, TemporalExtent::Instant { at }) => {
            end.is_none_or(|end| *at < end) && start.is_none_or(|start| *at >= start)
        }
        (
            TemporalExtent::Interval {
                start: left_start,
                end: left_end,
            },
            TemporalExtent::Interval {
                start: right_start,
                end: right_end,
            },
        ) => {
            left_end.is_none_or(|end| right_start.is_none_or(|start| start < end))
                && right_end.is_none_or(|end| left_start.is_none_or(|start| start < end))
        }
    }
}

fn contains_extent(parent: &TemporalExtent, child: &TemporalExtent) -> bool {
    match (parent, child) {
        (TemporalExtent::Unknown, _) => true,
        (_, TemporalExtent::Unknown) => true,
        (TemporalExtent::Instant { at: parent }, TemporalExtent::Instant { at: child }) => {
            parent == child
        }
        (TemporalExtent::Interval { start, end }, TemporalExtent::Instant { at }) => {
            start.is_none_or(|value| *at >= value) && end.is_none_or(|value| *at < value)
        }
        (
            TemporalExtent::Interval {
                start: parent_start,
                end: parent_end,
            },
            TemporalExtent::Interval {
                start: child_start,
                end: child_end,
            },
        ) => {
            parent_start.is_none_or(|value| child_start.is_some_and(|child| child >= value))
                && parent_end.is_none_or(|value| child_end.is_some_and(|child| child <= value))
        }
        _ => false,
    }
}
