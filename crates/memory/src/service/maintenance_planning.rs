// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::BTreeSet;

const MAX_REPAIR_MEMBERS: usize = 2048;

#[derive(Debug)]
pub struct MaintenanceScope {
    pub status: String,
    pub episodes: Vec<EpisodeView>,
    pub journal: Option<JournalView>,
    pub occurrences: Vec<OccurrenceId>,
    pub next_due: Option<DateTime<Utc>>,
    pub problem: Option<String>,
}
impl MaintenanceScope {
    fn empty(status: &str) -> Self {
        Self {
            status: status.into(),
            episodes: vec![],
            journal: None,
            occurrences: vec![],
            next_due: None,
            problem: None,
        }
    }
}

impl MemoryService {
    pub async fn plan_consolidation_scope(
        &self,
        subject: SubjectId,
        kind: &str,
        scope: &str,
    ) -> Result<MaintenanceScope> {
        let mut result = MaintenanceScope::empty("ready");
        match kind {
            "episode_revision" => {
                let revision = EpisodeRevisionId(scope.parse().map_err(|_| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "invalid Episode consolidation scope",
                    )
                })?);
                let episode = match self.episode_revision(subject, revision).await {
                    Ok(value) => value,
                    Err(Error {
                        code: nous_core::ErrorCode::NotFound,
                        ..
                    }) => return Ok(MaintenanceScope::empty("obsolete")),
                    Err(error) => return Err(error),
                };
                if episode.header.current_revision_id() != revision.0 || !episode_eligible(&episode)
                {
                    return Ok(MaintenanceScope::empty("obsolete"));
                }
                result.episodes.push(episode);
            }
            "journal_revision" => {
                let revision = JournalRevisionId(scope.parse().map_err(|_| {
                    Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "invalid Journal consolidation scope",
                    )
                })?);
                let journal = match self.journal_revision(subject, revision).await {
                    Ok(value) => value,
                    Err(Error {
                        code: nous_core::ErrorCode::NotFound,
                        ..
                    }) => return Ok(MaintenanceScope::empty("obsolete")),
                    Err(error) => return Err(error),
                };
                if journal.header.current_revision_id() != revision.0
                    || journal.header.lifecycle.acceptance != AcceptanceState::Accepted
                    || journal.header.lifecycle.integrity != IntegrityState::Valid
                    || journal.header.lifecycle.suppression != SuppressionState::Normal
                    || journal.header.lifecycle.purge != PurgeState::Normal
                {
                    return Ok(MaintenanceScope::empty("obsolete"));
                }
                for source in &journal.sources {
                    if let CognitiveRef::EpisodeRevision(id) = source {
                        result
                            .episodes
                            .push(self.episode_revision(subject, *id).await?);
                    }
                }
                result.journal = Some(journal);
            }
            _ => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid consolidation source kind",
                ));
            }
        }
        result.occurrences = scope_occurrences(&result.episodes)?;
        Ok(result)
    }
    pub async fn plan_episode_review(
        &self,
        subject: SubjectId,
        scope_kind: &str,
        scope: &str,
        trigger: i64,
    ) -> Result<MaintenanceScope> {
        let config = self.configuration.snapshot_for_subject(subject)?;
        let max_neighbors = config.get(nous_runtime::EPISODE_MAX_NEIGHBORS)? as i64;
        let neighbor_span =
            chrono::Duration::seconds(config.get(nous_runtime::EPISODE_NEIGHBOR_SPAN)? as i64);
        let (track, anchor, late, parent) = if scope_kind == "episode_revision" {
            let revision = EpisodeRevisionId(scope.parse().map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "invalid Episode review scope",
                )
                .with_source(error)
            })?);
            let source = match self.episode_revision(subject, revision).await {
                Ok(value) => value,
                Err(Error {
                    code: nous_core::ErrorCode::NotFound,
                    ..
                }) => return Ok(MaintenanceScope::empty("obsolete")),
                Err(error) => return Err(error),
            };
            if source.header.current_revision_id() != revision.0 || !episode_eligible(&source) {
                return Ok(MaintenanceScope::empty("obsolete"));
            }
            (
                source.header.details.track_key().to_owned(),
                extent_start(&source.revision.experience_time),
                None,
                source.revision.parent_episode_revision_id.map(|id| id.0),
            )
        } else if scope_kind == "track" {
            let late = sqlx::query("SELECT e.occurrence_id,e.observed_at FROM experience_items e WHERE e.subject_id=$1 AND e.recorded_seq<=$2 AND NOT EXISTS(SELECT 1 FROM episode_revision_members m JOIN cognition_revision_headers h ON h.revision_id=m.episode_revision_id AND h.object_kind='episode' JOIN cognition_objects o ON o.subject_id=h.subject_id AND o.object_kind=h.object_kind AND o.object_id=h.object_id WHERE o.subject_id=$1 AND o.track_key=$3 AND o.acceptance_state='accepted' AND m.ref_kind='occurrence' AND m.ref_value=e.occurrence_id::text) AND NOT EXISTS(SELECT 1 FROM episode_draft_members m JOIN episode_drafts d USING(draft_id) WHERE m.occurrence_id=e.occurrence_id AND d.state IN ('open','ready')) ORDER BY e.recorded_seq LIMIT 1")
                .bind(subject.0).bind(trigger).bind(scope).fetch_optional(self.store.pool()).await.map_err(db)?;
            let Some(late) = late else {
                return Ok(MaintenanceScope::empty("obsolete"));
            };
            (
                scope.to_owned(),
                Some(late.get::<DateTime<Utc>, _>("observed_at")),
                Some(OccurrenceId(late.get("occurrence_id"))),
                None,
            )
        } else {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "invalid Episode review scope kind",
            ));
        };
        let Some(anchor) = anchor else {
            return Ok(MaintenanceScope::empty("obsolete"));
        };
        // The local partition contains overlapping Episodes and the immediate
        // neighbors on either side. Age relative to today's clock is irrelevant.
        let ids: Vec<Uuid> = sqlx::query_scalar("WITH eligible AS (SELECT h.revision_id AS id,r.experience_time_start AS start_at,COALESCE(r.experience_time_end,r.experience_time_start) AS end_at FROM cognition_objects o JOIN cognition_revision_headers h ON h.subject_id=o.subject_id AND h.object_kind=o.object_kind AND h.object_id=o.object_id AND h.revision_id=o.current_revision_id JOIN episode_revisions r ON r.episode_revision_id=h.revision_id WHERE o.subject_id=$1 AND o.object_kind='episode' AND o.track_key=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND r.parent_episode_revision_id IS NOT DISTINCT FROM $3::uuid), neighborhood AS (SELECT id FROM eligible WHERE start_at<=$4 AND end_at>=$4 UNION SELECT id FROM (SELECT id FROM eligible WHERE end_at<$4 ORDER BY end_at DESC,id LIMIT 1) preceding UNION SELECT id FROM (SELECT id FROM eligible WHERE start_at>$4 ORDER BY start_at,id LIMIT 1) following) SELECT id FROM neighborhood LIMIT $5")
            .bind(subject.0).bind(&track).bind(parent).bind(anchor).bind(max_neighbors+1).fetch_all(self.store.pool()).await.map_err(db)?;
        if ids.len() as i64 > max_neighbors {
            let mut result = MaintenanceScope::empty("blocked");
            result.problem = Some("historical_repair_scope_exceeded".into());
            return Ok(result);
        }
        let mut result = MaintenanceScope::empty("ready");
        for id in ids {
            result.episodes.push(
                self.episode_revision(subject, EpisodeRevisionId(id))
                    .await?,
            );
        }
        result.episodes.sort_by_key(|episode| {
            (
                extent_start(&episode.revision.experience_time),
                episode.header.episode_id().0,
            )
        });
        if let (Some(first), Some(last)) = (
            result
                .episodes
                .first()
                .and_then(|episode| extent_start(&episode.revision.experience_time)),
            result
                .episodes
                .last()
                .and_then(|episode| extent_end(&episode.revision.experience_time)),
        ) && last.max(anchor) - first.min(anchor) > neighbor_span
        {
            result.status = "blocked".into();
            result.problem = Some("historical_repair_scope_exceeded".into());
            return Ok(result);
        }
        let member_count: usize = result
            .episodes
            .iter()
            .map(|episode| episode.members.len())
            .sum::<usize>()
            + usize::from(late.is_some());
        if member_count > MAX_REPAIR_MEMBERS {
            result.status = "blocked".into();
            result.problem = Some("historical_repair_scope_exceeded".into());
            return Ok(result);
        }
        result.occurrences = self
            .ordered_scope_occurrences(subject, &result.episodes, late)
            .await?;
        if result.episodes.is_empty() {
            result.status = "blocked".into();
            result.problem = Some("episode_neighborhood_unavailable".into());
        }
        Ok(result)
    }

    pub async fn plan_journal_review(
        &self,
        subject: SubjectId,
        kind: &str,
        scope: &str,
    ) -> Result<MaintenanceScope> {
        let config = self.configuration.snapshot_for_subject(subject)?;
        let now = self.cognition.now(subject);
        let max_count = config.get(super::longitudinal_policy::JOURNAL_MAX_EPISODES)? as i64;
        let span = chrono::Duration::seconds(
            config.get(super::longitudinal_policy::JOURNAL_MAX_SPAN)? as i64,
        );
        let delay = chrono::Duration::seconds(config.get(nous_runtime::SETTLE_DELAY_KEY)? as i64);
        let mut result = MaintenanceScope::empty("ready");
        let ids: Vec<Uuid> = if kind == "journal_revalidate" {
            let journal = match self
                .journal(
                    subject,
                    JournalId(scope.parse().map_err(|error| {
                        Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InvalidRequest,
                            "invalid Journal scope",
                        )
                        .with_source(error)
                    })?),
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(Error {
                    code: nous_core::ErrorCode::NotFound,
                    ..
                }) => return Ok(MaintenanceScope::empty("obsolete")),
                Err(error) => return Err(error),
            };
            if journal.header.lifecycle.purge != PurgeState::Normal
                || journal.header.lifecycle.acceptance != AcceptanceState::Accepted
                || journal.header.lifecycle.integrity == IntegrityState::Valid
            {
                return Ok(MaintenanceScope::empty("obsolete"));
            }
            let sources: Vec<String> = journal
                .sources
                .iter()
                .filter_map(|source| match source {
                    CognitiveRef::EpisodeRevision(id) => Some(id.0.to_string()),
                    _ => None,
                })
                .collect();
            let ids = sqlx::query_scalar("WITH RECURSIVE lineage(revision) AS (SELECT revision_id FROM cognition_revision_headers WHERE subject_id=$1 AND object_kind='episode' AND revision_id::text=ANY($2::text[]) UNION SELECT h.revision_id FROM cognition_revision_headers h JOIN lineage l ON h.parent_revision_id=l.revision OR EXISTS(SELECT 1 FROM episode_revision_relations rel WHERE rel.from_revision_id=h.revision_id AND rel.to_revision_id=l.revision AND rel.relation IN ('derived_from','split_from','merged_from')) WHERE h.subject_id=$1 AND h.object_kind='episode') SELECT o.current_revision_id FROM cognition_objects o JOIN lineage l ON l.revision=o.current_revision_id WHERE o.subject_id=$1 AND o.object_kind='episode' AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' ORDER BY o.created_at,o.object_id LIMIT $3")
                .bind(subject.0).bind(sources).bind(max_count+1).fetch_all(self.store.pool()).await.map_err(db)?;
            result.journal = Some(journal);
            ids
        } else {
            sqlx::query_scalar("SELECT h.revision_id FROM cognition_objects o JOIN cognition_revision_headers h ON h.subject_id=o.subject_id AND h.object_kind=o.object_kind AND h.object_id=o.object_id AND h.revision_id=o.current_revision_id JOIN episode_revisions r ON r.episode_revision_id=h.revision_id WHERE o.subject_id=$1 AND o.object_kind='episode' AND o.track_key=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND NOT EXISTS(SELECT 1 FROM journal_revision_sources s JOIN cognition_revision_headers jh ON jh.revision_id=s.journal_revision_id AND jh.object_kind='journal' JOIN cognition_objects j ON j.subject_id=jh.subject_id AND j.object_kind=jh.object_kind AND j.object_id=jh.object_id AND j.current_revision_id=jh.revision_id JOIN cognition_revision_headers old ON old.object_kind='episode' AND old.revision_id::text=s.ref_value WHERE j.subject_id=$1 AND j.acceptance_state='accepted' AND s.ref_kind='episode_revision' AND (old.object_id=o.object_id OR EXISTS(SELECT 1 FROM episode_revision_relations rel WHERE rel.from_revision_id=r.episode_revision_id AND rel.to_revision_id=old.revision_id))) ORDER BY r.experience_time_start,o.created_at,o.object_id LIMIT $3")
                .bind(subject.0).bind(scope).bind(max_count).fetch_all(self.store.pool()).await.map_err(db)?
        };
        if ids.len() as i64 > max_count {
            result.status = "blocked".into();
            result.problem = Some("journal_scope_bound_exceeded".into());
            return Ok(result);
        }
        let mut scope_start: Option<DateTime<Utc>> = None;
        let mut scope_end: Option<DateTime<Utc>> = None;
        for id in ids {
            let episode = self
                .episode_revision(subject, EpisodeRevisionId(id))
                .await?;
            if episode.revision.header.recorded_at + delay > now {
                result.next_due = Some(episode.revision.header.recorded_at + delay);
                if result.journal.is_some() {
                    result.status = "deferred".into();
                    result.problem = Some("journal_sources_settling".into());
                    return Ok(result);
                }
                break;
            }
            if let (Some(start), Some(end)) = (
                extent_start(&episode.revision.experience_time),
                extent_end(&episode.revision.experience_time),
            ) && end - start > span
            {
                result.status = "blocked".into();
                result.problem = Some("journal_scope_span_exceeded".into());
                return Ok(result);
            }
            if let Some(start) = extent_start(&episode.revision.experience_time) {
                scope_start = Some(scope_start.map_or(start, |old| old.min(start)));
            }
            if let Some(end) = extent_end(&episode.revision.experience_time) {
                scope_end = Some(scope_end.map_or(end, |old| old.max(end)));
            }
            if let (Some(start), Some(end)) = (scope_start, scope_end)
                && end - start > span
            {
                if result.journal.is_some() {
                    result.status = "blocked".into();
                    result.problem = Some("journal_scope_span_exceeded".into());
                    return Ok(result);
                }
                break;
            }
            result.episodes.push(episode);
        }
        if result.episodes.is_empty() {
            result.status = if result.journal.is_some() && result.next_due.is_none() {
                "withdraw"
            } else if result.next_due.is_some() {
                "deferred"
            } else {
                "obsolete"
            }
            .into();
        }
        result.occurrences = scope_occurrences(&result.episodes)?;
        Ok(result)
    }

    async fn ordered_scope_occurrences(
        &self,
        subject: SubjectId,
        episodes: &[EpisodeView],
        late: Option<OccurrenceId>,
    ) -> Result<Vec<OccurrenceId>> {
        let mut occurrences = BTreeSet::new();
        for episode in episodes {
            for member in &episode.members {
                if let CognitiveRef::Occurrence(id) = member.reference {
                    occurrences.insert(id.0);
                } else {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "automatic review requires occurrence members",
                    ));
                }
            }
        }
        if let Some(late) = late {
            occurrences.insert(late.0);
        }
        if occurrences.len() > 2048 {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "maintenance experience scope exceeds 2048 members",
            ));
        }
        let ids: Vec<Uuid> = occurrences.into_iter().collect();
        let ordered: Vec<Uuid>=sqlx::query_scalar("SELECT occurrence_id FROM experience_items WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[]) ORDER BY observed_at,recorded_seq")
            .bind(subject.0).bind(&ids).fetch_all(self.store.pool()).await.map_err(db)?;
        if ordered.len() != ids.len() {
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "maintenance members require Session experience",
            ));
        }
        Ok(ordered.into_iter().map(OccurrenceId).collect())
    }
}
fn scope_occurrences(episodes: &[EpisodeView]) -> Result<Vec<OccurrenceId>> {
    let occurrences: BTreeSet<_> = episodes
        .iter()
        .flat_map(|episode| &episode.members)
        .filter_map(|member| match member.reference {
            CognitiveRef::Occurrence(id) => Some(id),
            _ => None,
        })
        .collect();
    if occurrences.len() > 2048 {
        return Err(Error::new(
            nous_core::ErrorOwner::Memory,
            nous_core::ErrorCode::InvalidRequest,
            "maintenance scope exceeds 2048 occurrence members",
        ));
    }
    Ok(occurrences.into_iter().collect())
}

fn episode_eligible(episode: &EpisodeView) -> bool {
    episode.header.lifecycle.acceptance == AcceptanceState::Accepted
        && episode.header.lifecycle.integrity == IntegrityState::Valid
        && episode.header.lifecycle.suppression == SuppressionState::Normal
        && episode.header.lifecycle.purge == PurgeState::Normal
}

fn extent_start(time: &TemporalExtent) -> Option<DateTime<Utc>> {
    match time {
        TemporalExtent::Instant { at } => Some(*at),
        TemporalExtent::Interval { start, .. } => *start,
        TemporalExtent::Unknown => None,
    }
}
fn extent_end(time: &TemporalExtent) -> Option<DateTime<Utc>> {
    match time {
        TemporalExtent::Instant { at } => Some(*at),
        TemporalExtent::Interval { end, .. } => *end,
        TemporalExtent::Unknown => None,
    }
}
