//! Atomic local replacement of automatic experience organization.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodePartitionSource {
    pub revision: EpisodeRevisionId,
    pub expected_epoch: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodePartitionSegment {
    pub member_indices: Vec<usize>,
    pub title: Option<String>,
    pub boundary_explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodePartitionInput {
    pub operation_id: OperationId,
    pub subject: SubjectId,
    pub expected_authority_seq: i64,
    pub sources: Vec<EpisodePartitionSource>,
    pub ordered_occurrences: Vec<OccurrenceId>,
    pub segments: Vec<EpisodePartitionSegment>,
    pub producer: Option<ProducerSignature>,
}

impl MemoryService {
    pub async fn apply_episode_partition(
        &self,
        input: EpisodePartitionInput,
    ) -> Result<Vec<EpisodeView>> {
        validate_partition(&input)?;
        let started = self.cognition.now(input.subject);
        let digest = operation_digest("episode_partition", input.subject, &input)?;
        let mut mutation = match self
            .start_mutation(
                input.subject,
                input.operation_id,
                "episode_partition",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state != "committed" {
                    return Err(Error::Unavailable(
                        "Episode partition is in progress".into(),
                    ));
                }
                let revisions: Vec<EpisodeRevisionId> =
                    serde_json::from_str(&receipt.result_ref.ok_or_else(|| {
                        Error::Infrastructure("partition receipt has no revisions".into())
                    })?)
                    .map_err(|error| Error::Infrastructure(error.to_string()))?;
                return self.partition_views(input.subject, revisions).await;
            }
            MutationStart::Active(mutation) => mutation,
        };
        let sources = lock_sources(mutation.tx(), &input).await?;
        let watermark: i64 =
            sqlx::query_scalar("SELECT authority_seq FROM subjects WHERE subject_id=$1 FOR UPDATE")
                .bind(input.subject.0)
                .fetch_one(&mut **mutation.tx())
                .await
                .map_err(db)?;
        if watermark != input.expected_authority_seq {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Episode partition snapshot is stale",
            )
            .into());
        }
        let first = &sources[0];
        let track: String = first.try_get("track_key").map_err(db)?;
        let occurrences = validate_partition_occurrences(mutation.tx(), &input).await?;
        let formed_at = self
            .formation_time_in(mutation.tx(), input.subject, input.operation_id, started)
            .await?;
        let recorded_at = self.cognition.now(input.subject);
        let one_to_one = input.sources.len() == 1 && input.segments.len() == 1;
        if one_to_one && unchanged_partition(mutation.tx(), &input, first).await? {
            let outputs = vec![input.sources[0].revision];
            commit_partition_result(mutation, &outputs).await?;
            return self.partition_views(input.subject, outputs).await;
        }
        if !one_to_one {
            let ids: Vec<Uuid> = sources.iter().map(|row| row.get("episode_id")).collect();
            sqlx::query("UPDATE episode_objects SET acceptance_state='withdrawn',object_epoch=object_epoch+1 WHERE subject_id=$1 AND episode_id=ANY($2::uuid[])")
                .bind(input.subject.0).bind(&ids).execute(&mut **mutation.tx()).await.map_err(db)?;
        }
        let producer_id = partition_producer_in(mutation.tx(), input.producer.as_ref()).await?;
        let outputs = self
            .write_partition_segments_in(
                mutation.tx(),
                &input,
                first,
                &occurrences,
                formed_at,
                recorded_at,
                producer_id,
            )
            .await?;
        let sequence = mutation.invalidate(ProjectionInvalidation::text()).await?;
        let source_objects: Vec<Uuid> = sources.iter().map(|row| row.get("episode_id")).collect();
        self.invalidate_object_dependents_in(
            mutation.tx(),
            input.subject,
            "episode",
            &source_objects,
            sequence,
            "source_repartitioned",
        )
        .await?;
        for revision in &outputs {
            self.enqueue_concept_in(
                mutation.tx(),
                input.subject,
                CognitiveRef::EpisodeRevision(*revision),
                sequence,
            )
            .await?;
            self.schedule_episode_in(
                mutation.tx(),
                input.subject,
                *revision,
                &track,
                sequence,
                false,
            )
            .await?;
        }
        commit_partition_result(mutation, &outputs).await?;
        self.partition_views(input.subject, outputs).await
    }

    async fn write_partition_segments_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &EpisodePartitionInput,
        first: &sqlx::postgres::PgRow,
        occurrences: &[DateTime<Utc>],
        formed_at: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
        producer_id: Option<Uuid>,
    ) -> Result<Vec<EpisodeRevisionId>> {
        let track: String = first.try_get("track_key").map_err(db)?;
        let parent: Option<Uuid> = first.try_get("parent_episode_revision_id").map_err(db)?;
        let one_to_one = input.sources.len() == 1 && input.segments.len() == 1;
        let mut outputs = Vec::with_capacity(input.segments.len());
        for segment in &input.segments {
            let mut payload = partition_payload(input, segment, &track, parent, occurrences)?;
            payload.producer_signature_id = producer_id;
            let episode = if one_to_one {
                EpisodeId(first.try_get("episode_id").map_err(db)?)
            } else {
                EpisodeId::new()
            };
            validate_parent_and_overlap(
                tx,
                input.subject,
                &track,
                parent.map(EpisodeRevisionId),
                &payload.experience_time,
                one_to_one.then_some(episode.0),
            )
            .await?;
            let revision = EpisodeRevisionId::new();
            let (kind, time_start, time_end) = temporal_columns(&payload.experience_time);
            let prior_revision =
                one_to_one.then(|| EpisodeRevisionId(first.get("episode_revision_id")));
            let revision_no = if one_to_one {
                first.try_get::<i32, _>("revision_no").map_err(db)? + 1
            } else {
                1
            };
            if !one_to_one {
                sqlx::query("INSERT INTO episode_objects(episode_id,subject_id,track_key,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,created_at) VALUES($1,$2,$3,$4,1,'accepted','valid','normal','normal',$5)")
                    .bind(episode.0).bind(input.subject.0).bind(&track).bind(revision.0).bind(recorded_at).execute(&mut **tx).await.map_err(db)?;
            }
            insert_episode_revision_with_intent(
                &self.store,
                tx,
                &payload,
                episode,
                revision,
                prior_revision,
                one_to_one.then(|| "resegment".into()),
                revision_no,
                kind,
                time_start,
                time_end,
                formed_at,
                recorded_at,
            )
            .await?;
            if one_to_one {
                sqlx::query("UPDATE episode_objects SET current_revision_id=$2,object_epoch=object_epoch+1,integrity_state='valid' WHERE episode_id=$1")
                    .bind(episode.0).bind(revision.0).execute(&mut **tx).await.map_err(db)?;
            } else {
                let relation = if input.sources.len() == 1 {
                    "split_from"
                } else if input.segments.len() == 1 {
                    "merged_from"
                } else {
                    "derived_from"
                };
                for source in &input.sources {
                    sqlx::query("INSERT INTO episode_revision_relations(from_revision_id,to_revision_id,relation,created_at) VALUES($1,$2,$3,$4)")
                        .bind(revision.0).bind(source.revision.0).bind(relation).bind(recorded_at).execute(&mut **tx).await.map_err(db)?;
                }
            }
            outputs.push(revision);
        }
        Ok(outputs)
    }

    async fn partition_views(
        &self,
        subject: SubjectId,
        revisions: Vec<EpisodeRevisionId>,
    ) -> Result<Vec<EpisodeView>> {
        let mut views = Vec::with_capacity(revisions.len());
        for revision in revisions {
            views.push(self.episode_revision(subject, revision).await?);
        }
        Ok(views)
    }
}

async fn partition_producer_in(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    producer: Option<&ProducerSignature>,
) -> Result<Option<Uuid>> {
    match producer {
        Some(producer) => Ok(Some(
            AuthorityStore::register_producer_in(tx, producer).await?,
        )),
        None => Ok(None),
    }
}

fn validate_partition(input: &EpisodePartitionInput) -> Result<()> {
    let flattened: Vec<usize> = input
        .segments
        .iter()
        .flat_map(|segment| segment.member_indices.iter().copied())
        .collect();
    if input
        .producer
        .as_ref()
        .is_some_and(|producer| producer.operation != CapabilityOperation::EpisodeSegmentationText)
        || input.sources.is_empty()
        || input.sources.len() > 8
        || input.segments.is_empty()
        || input.segments.len() > 16
        || input.ordered_occurrences.is_empty()
        || input.ordered_occurrences.len() > 2048
        || input
            .segments
            .iter()
            .any(|segment| segment.member_indices.is_empty() || segment.member_indices.len() > 256)
        || flattened != (0..input.ordered_occurrences.len()).collect::<Vec<_>>()
        || input
            .sources
            .iter()
            .map(|source| source.revision.0)
            .collect::<BTreeSet<_>>()
            .len()
            != input.sources.len()
        || input
            .ordered_occurrences
            .iter()
            .map(|id| id.0)
            .collect::<BTreeSet<_>>()
            .len()
            != input.ordered_occurrences.len()
    {
        return Err(Error::Invalid(
            "Episode partition must cover bounded ordered members exactly once".into(),
        ));
    }
    Ok(())
}

async fn lock_sources(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &EpisodePartitionInput,
) -> Result<Vec<sqlx::postgres::PgRow>> {
    let ids: Vec<Uuid> = input
        .sources
        .iter()
        .map(|source| source.revision.0)
        .collect();
    let rows = sqlx::query("SELECT o.*,r.episode_revision_id,r.revision_no,r.parent_episode_revision_id,r.title,r.boundary_explanation FROM episode_objects o JOIN episode_revisions r ON r.episode_id=o.episode_id WHERE o.subject_id=$1 AND r.episode_revision_id=ANY($2::uuid[]) ORDER BY o.episode_id FOR UPDATE OF o")
        .bind(input.subject.0).bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
    if rows.len() != ids.len() {
        return Err(nous_core::DomainError::new(
            nous_core::DomainErrorCode::StaleRevision,
            "Episode partition source disappeared",
        )
        .into());
    }
    let track: String = rows[0].try_get("track_key").map_err(db)?;
    let parent: Option<Uuid> = rows[0].try_get("parent_episode_revision_id").map_err(db)?;
    for row in &rows {
        let revision: Uuid = row.try_get("episode_revision_id").map_err(db)?;
        let expected = input
            .sources
            .iter()
            .find(|source| source.revision.0 == revision)
            .ok_or_else(|| Error::Internal("unbound Episode source".into()))?;
        if row.get::<Uuid, _>("current_revision_id") != revision
            || row.get::<i64, _>("object_epoch") != expected.expected_epoch
            || row.get::<String, _>("track_key") != track
            || row.get::<Option<Uuid>, _>("parent_episode_revision_id") != parent
            || row.get::<String, _>("acceptance_state") != "accepted"
            || row.get::<String, _>("integrity_state") != "valid"
            || row.get::<String, _>("suppression_state") != "normal"
            || row.get::<String, _>("purge_state") != "normal"
        {
            return Err(nous_core::DomainError::new(
                nous_core::DomainErrorCode::StaleRevision,
                "Episode partition source state is stale",
            )
            .into());
        }
    }
    let members = sqlx::query("SELECT episode_revision_id,ref_kind,ref_value FROM episode_revision_members WHERE episode_revision_id=ANY($1::uuid[]) ORDER BY episode_revision_id,ordinal")
        .bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
    let supplied: BTreeSet<String> = input
        .ordered_occurrences
        .iter()
        .map(|id| id.0.to_string())
        .collect();
    if members.iter().any(|member| {
        member.get::<String, _>("ref_kind") != "occurrence"
            || !supplied.contains(&member.get::<String, _>("ref_value"))
    }) {
        return Err(Error::Invalid(
            "automatic partition must retain every source occurrence".into(),
        ));
    }
    let positions: std::collections::HashMap<String, usize> = input
        .ordered_occurrences
        .iter()
        .enumerate()
        .map(|(index, id)| (id.0.to_string(), index))
        .collect();
    let mut last = std::collections::HashMap::<Uuid, usize>::new();
    for member in &members {
        let revision: Uuid = member.try_get("episode_revision_id").map_err(db)?;
        let value: String = member.try_get("ref_value").map_err(db)?;
        let position = *positions
            .get(&value)
            .ok_or_else(|| Error::Invalid("partition omitted source member".into()))?;
        if last
            .insert(revision, position)
            .is_some_and(|prior| prior >= position)
        {
            return Err(Error::Invalid("partition reordered source members".into()));
        }
    }
    Ok(rows)
}

async fn unchanged_partition(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &EpisodePartitionInput,
    source: &sqlx::postgres::PgRow,
) -> Result<bool> {
    let original: Vec<String> = sqlx::query_scalar("SELECT ref_value FROM episode_revision_members WHERE episode_revision_id=$1 ORDER BY ordinal")
        .bind(input.sources[0].revision.0).fetch_all(&mut **tx).await.map_err(db)?;
    Ok(original
        == input
            .ordered_occurrences
            .iter()
            .map(|id| id.0.to_string())
            .collect::<Vec<_>>()
        && source.try_get::<Option<String>, _>("title").map_err(db)? == input.segments[0].title
        && source
            .try_get::<String, _>("boundary_explanation")
            .map_err(db)?
            == input.segments[0].boundary_explanation)
}

async fn validate_partition_occurrences(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: &EpisodePartitionInput,
) -> Result<Vec<DateTime<Utc>>> {
    let ids: Vec<Uuid> = input.ordered_occurrences.iter().map(|id| id.0).collect();
    let rows = sqlx::query("SELECT o.occurrence_id,o.observed_at FROM observation_occurrences o JOIN experience_items e ON e.occurrence_id=o.occurrence_id AND e.subject_id=o.subject_id WHERE o.subject_id=$1 AND o.occurrence_id=ANY($2::uuid[]) ORDER BY o.observed_at,e.recorded_seq FOR SHARE OF o,e")
        .bind(input.subject.0).bind(&ids).fetch_all(&mut **tx).await.map_err(db)?;
    if rows.len() != ids.len() {
        return Err(Error::FailedPrecondition(
            "partition occurrence is foreign or missing".into(),
        ));
    }
    let ordered: Vec<Uuid> = rows.iter().map(|row| row.get("occurrence_id")).collect();
    if ordered != ids {
        return Err(Error::Invalid(
            "partition members must follow experience chronology".into(),
        ));
    }
    let times: std::collections::HashMap<Uuid, DateTime<Utc>> = rows
        .into_iter()
        .map(|row| (row.get("occurrence_id"), row.get("observed_at")))
        .collect();
    ids.into_iter()
        .map(|id| {
            times
                .get(&id)
                .copied()
                .ok_or_else(|| Error::Internal("partition member time missing".into()))
        })
        .collect()
}

fn partition_payload(
    input: &EpisodePartitionInput,
    segment: &EpisodePartitionSegment,
    track: &str,
    parent: Option<Uuid>,
    occurrences: &[DateTime<Utc>],
) -> Result<EpisodeInput> {
    let occurrence_members: Vec<OccurrenceId> = segment
        .member_indices
        .iter()
        .map(|index| input.ordered_occurrences[*index])
        .collect();
    let times: Vec<DateTime<Utc>> = segment
        .member_indices
        .iter()
        .map(|index| occurrences[*index])
        .collect();
    let start = times
        .iter()
        .min()
        .copied()
        .ok_or_else(|| Error::Invalid("empty partition segment".into()))?;
    let end = times
        .iter()
        .max()
        .copied()
        .ok_or_else(|| Error::Invalid("empty partition segment".into()))?;
    let experience_time = if start == end {
        TemporalExtent::Instant { at: start }
    } else {
        TemporalExtent::Interval {
            start: Some(start),
            end: Some(end),
        }
    };
    let payload = EpisodeInput {
        operation_id: input.operation_id,
        subject: input.subject,
        track_key: track.to_owned(),
        title: segment.title.clone(),
        parent_episode_revision_id: parent.map(EpisodeRevisionId),
        experience_time,
        boundary_explanation: segment.boundary_explanation.clone(),
        producer_signature_id: None,
        members: occurrence_members
            .iter()
            .map(|id| EpisodeMemberInput {
                reference: CognitiveRef::Occurrence(*id),
                role: "experience".into(),
            })
            .collect(),
        basis: occurrence_members
            .iter()
            .map(|id| {
                RevisionBasis::Evidence(EvidenceRef {
                    epistemic_relation: None,
                    occurrence_id: *id,
                    locator: EvidenceLocator::WholeOccurrence,
                    basis_role: BasisRole::Direct,
                })
            })
            .collect(),
    };
    validate_episode_input(
        track,
        &payload.title,
        &payload.experience_time,
        &payload.boundary_explanation,
        &payload.members,
        &payload.basis,
    )?;
    Ok(payload)
}

async fn commit_partition_result(
    mut mutation: nous_persistence::MutationEnvelope<'_>,
    outputs: &[EpisodeRevisionId],
) -> Result<()> {
    let result =
        serde_json::to_string(outputs).map_err(|error| Error::Infrastructure(error.to_string()))?;
    mutation
        .publish_workflow_results(
            "memory",
            &outputs
                .iter()
                .copied()
                .map(CognitiveRef::EpisodeRevision)
                .collect::<Vec<_>>(),
        )
        .await?;
    mutation
        .commit("episode_partition", Some(&result), None, None)
        .await
}
