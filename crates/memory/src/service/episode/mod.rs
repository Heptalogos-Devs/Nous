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
pub struct EpisodeInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub track_key: String,
    pub title: Option<String>,
    pub parent_episode_revision_id: Option<EpisodeRevisionId>,
    pub experience_time: TemporalExtent,
    pub boundary_explanation: String,

    pub producer_signature_id: Option<Uuid>,
    pub members: Vec<EpisodeMemberInput>,
    pub basis: Vec<RevisionBasis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviseEpisodeInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub episode_id: EpisodeId,
    pub expected_object_epoch: i64,
    pub intent: String,
    pub title: Option<String>,
    pub parent_episode_revision_id: Option<EpisodeRevisionId>,
    pub experience_time: TemporalExtent,
    pub boundary_explanation: String,

    pub producer_signature_id: Option<Uuid>,
    pub members: Vec<EpisodeMemberInput>,
    pub basis: Vec<RevisionBasis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRevision {
    pub header: CognitionRevisionHeader,
    pub revision_intent: Option<String>,
    pub title: Option<String>,
    pub parent_episode_revision_id: Option<EpisodeRevisionId>,
    pub experience_time: TemporalExtent,
    pub boundary_explanation: String,
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
    pub header: CognitionHeader,
    pub revision: EpisodeRevision,
    pub members: Vec<EpisodeMember>,
    pub basis: Vec<RevisionBasis>,
    pub relations: Vec<EpisodeRelation>,
}

fn validate_episode_input(
    track: &str,
    title: &Option<String>,
    time: &TemporalExtent,
    boundary: &str,
    members: &[EpisodeMemberInput],
    basis: &[RevisionBasis],
) -> Result<()> {
    if !track.is_empty() && (track.len() > 128 || track.chars().any(char::is_control)) {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "Episode track_key is out of bounds",
        ));
    }
    if title.as_ref().is_some_and(|value| value.len() > 8192) {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "Episode title is out of bounds",
        ));
    }
    time.validate()?;
    if boundary.trim().is_empty() || boundary.len() > 16384 {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "Episode boundary explanation is out of bounds",
        ));
    }
    if members.is_empty() || members.len() > 2048 {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "Episode members are out of bounds",
        ));
    }
    if basis.is_empty() || basis.len() > 256 {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "Episode basis are out of bounds",
        ));
    }
    let mut keys = BTreeSet::new();
    for member in members {
        if member.role.is_empty()
            || member.role.len() > 128
            || !keys.insert(member.reference.to_string())
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Episode members must be unique and bounded",
            ));
        }
    }
    validate_exact_basis(basis)
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid Episode member reference",
            ));
        }
        if !store
            .reference_in_subject_tx(tx, subject, &member.reference)
            .await?
        {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::FailedPrecondition,
                "Episode member is outside Subject",
            ));
        }
    }
    Ok(())
}

async fn validate_episode_basis_in_tx(
    store: &AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    basis: &[RevisionBasis],
) -> Result<()> {
    for basis in basis {
        match basis {
            RevisionBasis::Evidence(value) => {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2)").bind(subject.0).bind(value.occurrence_id.0).fetch_one(&mut **tx).await.map_err(db)?;
                if !exists {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Episode evidence is outside Subject",
                    ));
                }
            }
            RevisionBasis::CognitionDependency(value) => {
                if !matches!(
                    value.target_revision,
                    CognitiveRef::MemoryRevision(_)
                        | CognitiveRef::CognitiveSchemaRevision(_)
                        | CognitiveRef::EpisodeRevision(_)
                ) {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Episode support must target an exact revision",
                    ));
                }
                if !store
                    .reference_in_subject_tx(tx, subject, &value.target_revision)
                    .await?
                {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Episode support is outside Subject",
                    ));
                }
            }
            RevisionBasis::Seed(_) => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "Episode cannot use Cognitive Seed support",
                ));
            }
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
        let parent_time = sqlx::query("SELECT experience_time_kind,experience_time_start,experience_time_end FROM episode_revisions r JOIN cognition_revision_headers h ON h.revision_id=r.episode_revision_id WHERE r.episode_revision_id=$1 AND h.subject_id=$2 AND h.object_kind='episode'").bind(parent.0).bind(subject.0).fetch_optional(&mut **tx).await.map_err(db)?.ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InvalidRequest, "Episode parent is outside Subject"))?;
        let parent_extent = temporal_from_columns(
            ErrorOwner::Memory,
            parent_time.try_get("experience_time_kind").map_err(db)?,
            parent_time.try_get("experience_time_start").map_err(db)?,
            parent_time.try_get("experience_time_end").map_err(db)?,
        )?;
        if !contains_extent(&parent_extent, time) {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Episode child lies outside parent experience span",
            ));
        }
    }
    let rows = sqlx::query("SELECT r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM cognition_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.object_kind='episode' AND o.subject_id=$1 AND o.track_key=$2 AND o.acceptance_state='accepted' AND r.parent_episode_revision_id IS NOT DISTINCT FROM $3 AND ($4::uuid IS NULL OR o.object_id<>$4)").bind(subject.0).bind(track).bind(parent.map(|value| value.0)).bind(exclude).fetch_all(&mut **tx).await.map_err(db)?;
    for row in rows {
        let sibling = temporal_from_columns(
            ErrorOwner::Memory,
            row.try_get("experience_time_kind").map_err(db)?,
            row.try_get("experience_time_start").map_err(db)?,
            row.try_get("experience_time_end").map_err(db)?,
        )?;
        if known_overlap(&sibling, time) {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::Conflict,
                "same-track Episode sibling experience spans overlap",
            ));
        }
    }
    Ok(())
}

pub(super) async fn validate_episode_reaccept_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    episode: EpisodeId,
) -> Result<()> {
    let row = sqlx::query("SELECT o.track_key,r.parent_episode_revision_id,r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM cognition_objects o JOIN cognition_revision_headers h ON h.subject_id=o.subject_id AND h.object_kind=o.object_kind AND h.object_id=o.object_id AND h.revision_id=o.current_revision_id JOIN episode_revisions r ON r.episode_revision_id=h.revision_id WHERE o.object_kind='episode' AND o.subject_id=$1 AND o.object_id=$2")
        .bind(subject.0).bind(episode.0).fetch_one(&mut **tx).await.map_err(db)?;
    let time = temporal_from_columns(
        ErrorOwner::Memory,
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::Conflict,
                "Episode parent hierarchy contains a cycle",
            ));
        }
        let row = sqlx::query(
            "SELECT h.object_id AS episode_id,r.parent_episode_revision_id FROM episode_revisions r JOIN cognition_revision_headers h ON h.revision_id=r.episode_revision_id WHERE h.object_kind='episode' AND r.episode_revision_id=$1",
        )
        .bind(parent)
        .fetch_optional(&mut **tx)
        .await
        .map_err(db)?
        .ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InvalidRequest, "Episode parent revision does not exist"))?;
        let parent_episode: Uuid = row.try_get("episode_id").map_err(db)?;
        if parent_episode == episode {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::Conflict,
                "Episode parent hierarchy contains a cycle",
            ));
        }
        let next: Option<Uuid> = row.try_get("parent_episode_revision_id").map_err(db)?;
        let Some(next) = next else {
            return Ok(());
        };
        parent = next;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Episode creation binds one immutable revision and its object identity"
)]
async fn insert_episode_revision(
    store: &AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &EpisodeInput,
    episode: EpisodeId,
    revision: EpisodeRevisionId,
    parent_revision: Option<EpisodeRevisionId>,
    intent: Option<String>,
    kind: &str,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    formed_at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
) -> Result<()> {
    insert_episode_revision_with_intent(
        store,
        tx,
        input,
        episode,
        revision,
        parent_revision,
        intent,
        1,
        kind,
        start,
        end,
        formed_at,
        recorded_at,
    )
    .await
}

#[expect(
    clippy::too_many_arguments,
    reason = "Episode revision insertion receives the complete canonical payload"
)]
async fn insert_episode_revision_with_intent(
    store: &AuthorityStore,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &EpisodeInput,
    episode: EpisodeId,
    revision: EpisodeRevisionId,
    parent_revision: Option<EpisodeRevisionId>,
    intent: Option<String>,
    revision_no: i32,
    kind: &str,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    formed_at: DateTime<Utc>,
    recorded_at: DateTime<Utc>,
) -> Result<()> {
    MemoryService::insert_cognition_revision_header_in(
        tx,
        input.subject,
        &CognitionRevisionHeader::new(
            CognitionKind::Episode,
            episode.0,
            revision.0,
            revision_no,
            parent_revision.map(|value| value.0),
            formed_at,
            recorded_at,
            input.producer_signature_id,
        ),
    )
    .await?;
    sqlx::query("INSERT INTO episode_revisions(episode_revision_id,revision_intent,title,parent_episode_revision_id,experience_time_kind,experience_time_start,experience_time_end,boundary_explanation) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
        .bind(revision.0).bind(intent).bind(&input.title).bind(input.parent_episode_revision_id.map(|value| value.0)).bind(kind).bind(start).bind(end).bind(&input.boundary_explanation).execute(&mut **tx).await.map_err(db)?;
    for (ordinal, member) in input.members.iter().enumerate() {
        let (kind, value) = reference_parts(&member.reference);
        sqlx::query("INSERT INTO episode_revision_members(episode_revision_id,ordinal,ref_kind,ref_value,role) VALUES($1,$2,$3,$4,$5)").bind(revision.0).bind(ordinal as i32).bind(kind).bind(value).bind(&member.role).execute(&mut **tx).await.map_err(db)?;
    }
    for (basis_no, basis) in input.basis.iter().enumerate() {
        insert_episode_support(tx, revision, basis_no as i32, basis).await?;
    }
    store
        .ensure_identity_addresses_in(
            tx,
            input.subject,
            &[
                CognitiveRef::Episode(episode),
                CognitiveRef::EpisodeRevision(revision),
            ],
            input.title.as_deref().unwrap_or(""),
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "Episode cannot use Cognitive Seed support",
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
