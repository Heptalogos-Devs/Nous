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
    pub fn basis_catalog(&self) -> std::collections::BTreeMap<String, RevisionBasis> {
        let mut catalog = std::collections::BTreeMap::new();
        for episode in &self.episodes {
            let direct = RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: CognitiveRef::EpisodeRevision(
                    episode.revision.episode_revision_id,
                ),
                basis_role: BasisRole::Direct,
            });
            for basis in std::iter::once(direct)
                .chain(episode.basis.iter().cloned())
                .chain(
                    episode
                        .members
                        .iter()
                        .filter_map(super::consolidation_context::member_basis),
                )
            {
                catalog.insert(basis.canonical_key(), basis);
            }
        }
        catalog
    }

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
                let revision =
                    EpisodeRevisionId(scope.parse().map_err(|_| {
                        Error::Invalid("invalid Episode consolidation scope".into())
                    })?);
                let episode = match self.episode_revision(subject, revision).await {
                    Ok(value) => value,
                    Err(Error::NotFound(_)) => return Ok(MaintenanceScope::empty("obsolete")),
                    Err(error) => return Err(error),
                };
                if episode.object.current_revision_id != revision || !episode_eligible(&episode) {
                    return Ok(MaintenanceScope::empty("obsolete"));
                }
                result.episodes.push(episode);
            }
            "journal_revision" => {
                let revision =
                    JournalRevisionId(scope.parse().map_err(|_| {
                        Error::Invalid("invalid Journal consolidation scope".into())
                    })?);
                let journal = match self.journal_revision(subject, revision).await {
                    Ok(value) => value,
                    Err(Error::NotFound(_)) => return Ok(MaintenanceScope::empty("obsolete")),
                    Err(error) => return Err(error),
                };
                if journal.object.current_revision_id != revision
                    || journal.object.acceptance_state != AcceptanceState::Accepted
                    || journal.object.integrity_state != IntegrityState::Valid
                    || journal.object.suppression_state != SuppressionState::Normal
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
            _ => return Err(Error::Invalid("invalid consolidation source kind".into())),
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
            let revision = EpisodeRevisionId(
                scope
                    .parse()
                    .map_err(|_| Error::Invalid("invalid Episode review scope".into()))?,
            );
            let source = match self.episode_revision(subject, revision).await {
                Ok(value) => value,
                Err(Error::NotFound(_)) => return Ok(MaintenanceScope::empty("obsolete")),
                Err(error) => return Err(error),
            };
            if source.object.current_revision_id != revision || !episode_eligible(&source) {
                return Ok(MaintenanceScope::empty("obsolete"));
            }
            (
                source.object.track_key.clone(),
                extent_start(&source.revision.experience_time),
                None,
                source.revision.parent_episode_revision_id.map(|id| id.0),
            )
        } else if scope_kind == "track" {
            let late = sqlx::query("SELECT e.occurrence_id,e.observed_at FROM experience_items e WHERE e.subject_id=$1 AND e.recorded_seq<=$2 AND NOT EXISTS(SELECT 1 FROM episode_revision_members m JOIN episode_objects o ON o.current_revision_id=m.episode_revision_id WHERE o.subject_id=$1 AND o.track_key=$3 AND o.acceptance_state='accepted' AND m.ref_kind='occurrence' AND m.ref_value=e.occurrence_id::text) AND NOT EXISTS(SELECT 1 FROM episode_draft_members m JOIN episode_drafts d USING(draft_id) WHERE m.occurrence_id=e.occurrence_id AND d.state IN ('open','ready')) ORDER BY e.recorded_seq LIMIT 1")
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
            return Err(Error::Invalid("invalid Episode review scope kind".into()));
        };
        let Some(anchor) = anchor else {
            return Ok(MaintenanceScope::empty("obsolete"));
        };
        // The local partition contains overlapping Episodes and the immediate
        // neighbors on either side. Age relative to today's clock is irrelevant.
        let ids: Vec<Uuid> = sqlx::query_scalar("WITH eligible AS (SELECT o.current_revision_id AS id,r.experience_time_start AS start_at,COALESCE(r.experience_time_end,r.experience_time_start) AS end_at FROM episode_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.track_key=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND r.parent_episode_revision_id IS NOT DISTINCT FROM $3::uuid), neighborhood AS (SELECT id FROM eligible WHERE start_at<=$4 AND end_at>=$4 UNION SELECT id FROM (SELECT id FROM eligible WHERE end_at<$4 ORDER BY end_at DESC,id LIMIT 1) preceding UNION SELECT id FROM (SELECT id FROM eligible WHERE start_at>$4 ORDER BY start_at,id LIMIT 1) following) SELECT id FROM neighborhood LIMIT $5")
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
                episode.object.episode_id.0,
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
                    JournalId(
                        scope
                            .parse()
                            .map_err(|_| Error::Invalid("invalid Journal scope".into()))?,
                    ),
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(Error::NotFound(_)) => return Ok(MaintenanceScope::empty("obsolete")),
                Err(error) => return Err(error),
            };
            if journal.object.acceptance_state != AcceptanceState::Accepted
                || journal.object.integrity_state == IntegrityState::Valid
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
            let ids = sqlx::query_scalar("WITH RECURSIVE lineage(revision) AS (SELECT episode_revision_id FROM episode_revisions WHERE subject_id=$1 AND episode_revision_id::text=ANY($2::text[]) UNION SELECT r.episode_revision_id FROM episode_revisions r JOIN lineage l ON r.parent_revision_id=l.revision OR EXISTS(SELECT 1 FROM episode_revision_relations rel WHERE rel.from_revision_id=r.episode_revision_id AND rel.to_revision_id=l.revision AND rel.relation IN ('derived_from','split_from','merged_from')) WHERE r.subject_id=$1) SELECT o.current_revision_id FROM episode_objects o JOIN lineage l ON l.revision=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' ORDER BY o.created_at,o.episode_id LIMIT $3")
                .bind(subject.0).bind(sources).bind(max_count+1).fetch_all(self.store.pool()).await.map_err(db)?;
            result.journal = Some(journal);
            ids
        } else {
            sqlx::query_scalar("SELECT o.current_revision_id FROM episode_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.track_key=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND NOT EXISTS(SELECT 1 FROM journal_revision_sources s JOIN journal_objects j ON j.current_revision_id=s.journal_revision_id JOIN episode_revisions old ON old.episode_revision_id::text=s.ref_value WHERE j.subject_id=$1 AND j.acceptance_state='accepted' AND s.ref_kind='episode_revision' AND (old.episode_id=o.episode_id OR EXISTS(SELECT 1 FROM episode_revision_relations rel WHERE rel.from_revision_id=r.episode_revision_id AND rel.to_revision_id=old.episode_revision_id))) ORDER BY r.experience_time_start,o.created_at,o.episode_id LIMIT $3")
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
            if episode.revision.recorded_at + delay > now {
                result.next_due = Some(episode.revision.recorded_at + delay);
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
                    return Err(Error::Invalid(
                        "automatic review requires occurrence members".into(),
                    ));
                }
            }
        }
        if let Some(late) = late {
            occurrences.insert(late.0);
        }
        if occurrences.len() > 2048 {
            return Err(Error::Invalid(
                "maintenance experience scope exceeds 2048 members".into(),
            ));
        }
        let ids: Vec<Uuid> = occurrences.into_iter().collect();
        let ordered: Vec<Uuid>=sqlx::query_scalar("SELECT occurrence_id FROM experience_items WHERE subject_id=$1 AND occurrence_id=ANY($2::uuid[]) ORDER BY observed_at,recorded_seq")
            .bind(subject.0).bind(&ids).fetch_all(self.store.pool()).await.map_err(db)?;
        if ordered.len() != ids.len() {
            return Err(Error::Invalid(
                "maintenance members require Session experience".into(),
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
        return Err(Error::Invalid(
            "maintenance scope exceeds 2048 occurrence members".into(),
        ));
    }
    Ok(occurrences.into_iter().collect())
}

fn episode_eligible(episode: &EpisodeView) -> bool {
    episode.object.acceptance_state == AcceptanceState::Accepted
        && episode.object.integrity_state == IntegrityState::Valid
        && episode.object.suppression_state == SuppressionState::Normal
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

pub struct MaintenanceMember {
    pub occurrence: nous_material::ObservationOccurrence,
    pub experience: nous_runtime::ExperienceMetadata,
    pub text: String,
    pub text_partial: bool,
}
impl MemoryService {
    pub async fn maintenance_members(
        &self,
        subject: SubjectId,
        occurrences: &[OccurrenceId],
        member_bytes: usize,
    ) -> Result<(Vec<MaintenanceMember>, bool)> {
        let mut experience = self
            .cognition
            .experience_metadata(subject, occurrences)
            .await?;
        let mut sources = self
            .material
            .maintenance_sources(subject, occurrences)
            .await?;
        sources.sort_by_key(|(occurrence, _)| {
            (
                occurrence.observed_at,
                experience
                    .get(&occurrence.occurrence_id)
                    .and_then(|item| item.recorded_seq)
                    .unwrap_or(i64::MAX),
                occurrence.occurrence_id,
            )
        });
        let mut remaining = 65536usize;
        let mut exceeded = false;
        let mut members = Vec::with_capacity(sources.len());
        for (occurrence, reference) in sources {
            let excerpt = if remaining == 0 {
                exceeded = true;
                None
            } else {
                self.material
                    .text_excerpt(
                        subject,
                        &reference,
                        remaining.min(member_bytes) as u64,
                        None,
                    )
                    .await?
            };
            let (text, text_partial) = match excerpt {
                Some(view) => {
                    remaining = remaining.saturating_sub(view.text.len());
                    let partial = view.selection.start > 0 || view.selection.end < view.total_bytes;
                    (view.text, partial)
                }
                None => (String::new(), true),
            };
            let metadata = experience
                .remove(&occurrence.occurrence_id)
                .unwrap_or_default();
            members.push(MaintenanceMember {
                occurrence,
                experience: metadata,
                text,
                text_partial,
            });
        }
        Ok((members, exceeded))
    }
}
