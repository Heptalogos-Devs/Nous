// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EpisodeId, EpisodeRevisionId, OperationId, Result, SubjectId};
use nous_memory::{EpisodeInput, EpisodeMemberInput, ReviseEpisodeInput};

fn member(value: p::EpisodeMember) -> Result<EpisodeMemberInput> {
    Ok(EpisodeMemberInput {
        reference: from_ref(required(value.reference, "member.reference")?)?,
        role: if value.role.is_empty() {
            "member".into()
        } else {
            value.role
        },
    })
}

fn revision(
    value: nous_memory::EpisodeRevision,
    members: Vec<nous_memory::EpisodeMember>,
    basis: Vec<nous_memory::RevisionBasis>,
) -> p::EpisodeRevision {
    p::EpisodeRevision {
        episode_revision_id: value.episode_revision_id.0.to_string(),
        episode_id: value.episode_id.0.to_string(),
        subject_id: value.subject_id.0.to_string(),
        revision_no: value.revision_no,
        parent_revision_id: value.parent_revision_id.map(|v| v.0.to_string()),
        revision_intent: value.revision_intent,
        title: value.title,
        parent_episode_revision_id: value.parent_episode_revision_id.map(|v| v.0.to_string()),
        experience_time: Some(temporal_proto(&value.experience_time)),
        boundary_explanation: value.boundary_explanation,
        formed_at: Some(timestamp(value.formed_at)),
        recorded_at: Some(timestamp(value.recorded_at)),
        producer_signature_id: value.producer_signature_id.map(|v| v.to_string()),
        members: members
            .into_iter()
            .map(|member| p::EpisodeMember {
                ordinal: member.ordinal,
                reference: Some(to_ref(member.reference)),
                role: member.role,
            })
            .collect(),
        basis: basis.into_iter().map(basis_proto).collect(),
    }
}

pub(super) fn view(value: nous_memory::EpisodeView) -> p::Episode {
    let current = value.object.current_revision_id == value.revision.episode_revision_id;
    p::Episode {
        episode_id: value.object.episode_id.0.to_string(),
        subject_id: value.object.subject_id.0.to_string(),
        track_key: value.object.track_key,
        current_revision_id: value.object.current_revision_id.0.to_string(),
        object_epoch: value.object.object_epoch,
        acceptance_state: enum_name(value.object.acceptance_state),
        integrity_state: enum_name(value.object.integrity_state),
        suppression_state: enum_name(value.object.suppression_state),
        created_at: Some(timestamp(value.object.created_at)),
        current_revision: current.then(|| revision(value.revision, value.members, value.basis)),
    }
}

fn response(value: nous_memory::EpisodeView) -> p::EpisodeResponse {
    p::EpisodeResponse {
        episode: Some(view(value)),
        degradation: Vec::new(),
    }
}

fn parsed_parent(value: Option<String>) -> Result<Option<EpisodeRevisionId>> {
    value
        .as_deref()
        .map(|value| id(value).map(EpisodeRevisionId))
        .transpose()
}

impl KernelService {
    pub(super) async fn create_episode(
        &self,
        input: p::CreateEpisodeRequest,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let value = self
            .require_memory()?
            .create_episode(EpisodeInput {
                operation_id: OperationId(id(&input.operation_id)?),
                subject,
                track_key: input.track_key,
                title: input.title,
                parent_episode_revision_id: parsed_parent(input.parent_episode_revision_id)?,
                experience_time: temporal(input.experience_time)?,
                boundary_explanation: input.boundary_explanation,
                producer_signature_id: input
                    .producer_signature_id
                    .as_deref()
                    .map(id)
                    .transpose()?,
                members: input
                    .members
                    .into_iter()
                    .map(member)
                    .collect::<Result<_>>()?,
                basis: input.basis.into_iter().map(basis).collect::<Result<_>>()?,
            })
            .await?;
        Ok(response(value))
    }

    pub(super) async fn get_episode(&self, input: p::ObjectRequest) -> Result<p::EpisodeResponse> {
        Ok(response(
            self.require_memory()?
                .episode(
                    SubjectId(id(&input.subject_id)?),
                    EpisodeId(id(&input.id)?),
                    None,
                )
                .await?,
        ))
    }

    pub(super) async fn get_episode_revision(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::EpisodeRevision> {
        let value = self
            .require_memory()?
            .episode_revision(
                SubjectId(id(&input.subject_id)?),
                EpisodeRevisionId(id(&input.id)?),
            )
            .await?;
        Ok(revision(value.revision, value.members, value.basis))
    }

    pub(super) async fn list_episodes(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListEpisodesResponse> {
        Ok(p::ListEpisodesResponse {
            items: self
                .require_memory()?
                .list_episodes(SubjectId(id(&input.subject_id)?))
                .await?
                .into_iter()
                .map(view)
                .collect(),
            next_page_token: String::new(),
        })
    }

    pub(super) async fn list_episode_revisions(
        &self,
        input: p::ListEpisodeRevisionsRequest,
    ) -> Result<p::ListEpisodeRevisionsResponse> {
        let values = self
            .require_memory()?
            .episode_history(
                SubjectId(id(&input.subject_id)?),
                EpisodeId(id(&input.episode_id)?),
            )
            .await?;
        Ok(p::ListEpisodeRevisionsResponse {
            items: values
                .into_iter()
                .map(|value| revision(value.revision, value.members, value.basis))
                .collect(),
            next_page_token: String::new(),
        })
    }

    pub(super) async fn revise_episode(
        &self,
        input: p::ReviseEpisodeRequest,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        Ok(response(
            self.require_memory()?
                .revise_episode(ReviseEpisodeInput {
                    operation_id: OperationId(id(&input.operation_id)?),
                    subject,
                    episode_id: EpisodeId(id(&input.episode_id)?),
                    expected_object_epoch: input.expected_object_epoch,
                    intent: input.intent,
                    title: input.title,
                    parent_episode_revision_id: parsed_parent(input.parent_episode_revision_id)?,
                    experience_time: temporal(input.experience_time)?,
                    boundary_explanation: input.boundary_explanation,
                    producer_signature_id: input
                        .producer_signature_id
                        .as_deref()
                        .map(id)
                        .transpose()?,
                    members: input
                        .members
                        .into_iter()
                        .map(member)
                        .collect::<Result<_>>()?,
                    basis: input.basis.into_iter().map(basis).collect::<Result<_>>()?,
                })
                .await?,
        ))
    }

    pub(super) async fn link_episode_revisions(
        &self,
        input: p::LinkEpisodeRevisionsRequest,
    ) -> Result<()> {
        self.require_memory()?
            .link_episode_revisions(
                SubjectId(id(&input.subject_id)?),
                OperationId(id(&input.operation_id)?),
                EpisodeRevisionId(id(&input.from_revision_id)?),
                EpisodeRevisionId(id(&input.to_revision_id)?),
                input.relation,
            )
            .await
    }

    async fn episode_lifecycle(
        &self,
        input: p::EpisodeMutationRequest,
        operation: &str,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let episode = EpisodeId(id(&input.episode_id)?);
        let operation_id = OperationId(id(&input.operation_id)?);
        let value = match operation {
            "suppress" => {
                self.require_memory()?
                    .suppress_episode(subject, episode, operation_id, input.expected_object_epoch)
                    .await?
            }
            "restore" => {
                self.require_memory()?
                    .restore_episode(subject, episode, operation_id, input.expected_object_epoch)
                    .await?
            }
            "withdraw" => {
                self.require_memory()?
                    .withdraw_episode(subject, episode, operation_id, input.expected_object_epoch)
                    .await?
            }
            "reaccept" => {
                self.require_memory()?
                    .reaccept_episode(subject, episode, operation_id, input.expected_object_epoch)
                    .await?
            }
            _ => return Err(Error::Invalid("unknown Episode lifecycle operation".into())),
        };
        Ok(response(value))
    }

    pub(super) async fn suppress_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, "suppress").await
    }
    pub(super) async fn restore_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, "restore").await
    }
    pub(super) async fn withdraw_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, "withdraw").await
    }
    pub(super) async fn reaccept_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, "reaccept").await
    }

    pub(super) async fn purge_episode(&self, input: p::EpisodeMutationRequest) -> Result<()> {
        self.require_memory()?
            .purge_episode(
                SubjectId(id(&input.subject_id)?),
                EpisodeId(id(&input.episode_id)?),
                OperationId(id(&input.operation_id)?),
                input.expected_object_epoch,
            )
            .await
    }
}
