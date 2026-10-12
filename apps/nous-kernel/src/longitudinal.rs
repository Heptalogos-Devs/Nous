//! Composition of Runtime drafts with Memory Authority commits.

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::NousRuntime;
use chrono::{DateTime, Utc};
use nous_core::*;
use nous_memory::{
    BasisRole, EpisodeInput, EpisodeMemberInput, EpisodeView, EvidenceLocator, EvidenceRef,
    RevisionBasis,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceSegmentationResult {
    pub processed_count: usize,
    pub episodes: Vec<EpisodeView>,
    pub next_due: Option<DateTime<Utc>>,
}

impl NousRuntime {
    pub async fn organize_experience(
        &self,
        subject: SubjectId,
        limit: u32,
        close: bool,
    ) -> Result<ExperienceSegmentationResult> {
        let memory = self.require_memory()?;
        let progress = self
            .cognition
            .segment_experience(subject, "interaction", limit, close)
            .await?;
        let mut episodes = Vec::with_capacity(progress.ready.len());
        for draft in progress.ready {
            let experience_time = if draft.observed_start == draft.observed_end {
                TemporalExtent::Instant {
                    at: draft.observed_start,
                }
            } else {
                TemporalExtent::Interval {
                    start: Some(draft.observed_start),
                    end: Some(draft.observed_end),
                }
            };
            let episode = memory
                .create_episode(EpisodeInput {
                    operation_id: OperationId(uuid::Uuid::new_v5(
                        &uuid::Uuid::NAMESPACE_OID,
                        format!("episode-draft:{}", draft.draft_id).as_bytes(),
                    )),
                    subject,
                    track_key: draft.track_key,

                    content: nous_memory::EpisodeContent {
                        title: None,
                        parent_episode_revision_id: None,
                        experience_time,
                        boundary_explanation: draft
                            .boundary_reason
                            .unwrap_or_else(|| "experience_boundary".into()),
                        producer_signature_id: None,
                        members: draft
                            .members
                            .iter()
                            .map(|id| EpisodeMemberInput {
                                reference: CognitiveRef::Occurrence(*id),
                                role: "experience".into(),
                            })
                            .collect(),
                        basis: draft
                            .members
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
                    },
                })
                .await?;
            self.cognition
                .acknowledge_episode_draft(
                    subject,
                    draft.draft_id,
                    episode.revision.episode_revision_id,
                )
                .await?;
            episodes.push(episode);
        }
        Ok(ExperienceSegmentationResult {
            processed_count: progress.processed_count,
            episodes,
            next_due: progress.next_due,
        })
    }
}
