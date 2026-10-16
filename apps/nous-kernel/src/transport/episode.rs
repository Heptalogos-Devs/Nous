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

pub(super) fn revision(
    value: nous_memory::EpisodeRevision,
    members: Vec<nous_memory::EpisodeMember>,
    basis: Vec<nous_memory::RevisionBasis>,
    relations: Vec<nous_memory::EpisodeRelation>,
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
        experience_time: Some(domain_temporal_proto(
            &value.experience_time_domain_ref,
            &value.experience_time,
        )),
        boundary_explanation: value.boundary_explanation,
        formed_at: Some(formation_proto(
            &value.formed_time_domain_ref,
            value.formed_at,
            value.formed_recorded_at,
        )),
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
        relations: relations
            .into_iter()
            .map(|relation| p::EpisodeRelation {
                from_revision_id: relation.from_revision_id.0.to_string(),
                to_revision_id: relation.to_revision_id.0.to_string(),
                relation: relation.relation,
                created_at: Some(timestamp(relation.created_at)),
            })
            .collect(),
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
        current_revision: current
            .then(|| revision(value.revision, value.members, value.basis, value.relations)),
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

fn episode_content(content: p::EpisodeContent) -> Result<nous_memory::EpisodeContent> {
    Ok(nous_memory::EpisodeContent {
        title: content.title,
        parent_episode_revision_id: parsed_parent(content.parent_episode_revision_id)?,
        experience_time_domain_ref: required(content.experience_time.clone(), "experience_time")?
            .time_domain_ref,
        experience_time: domain_temporal(content.experience_time)?.extent,
        boundary_explanation: content.boundary_explanation,
        producer_signature_id: content
            .producer_signature_id
            .as_deref()
            .map(id)
            .transpose()?,
        members: content
            .members
            .into_iter()
            .map(member)
            .collect::<Result<_>>()?,
        basis: content
            .basis
            .into_iter()
            .map(basis)
            .collect::<Result<_>>()?,
    })
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

                content: episode_content(required(input.content, "content")?)?,
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
        Ok(revision(
            value.revision,
            value.members,
            value.basis,
            value.relations,
        ))
    }

    pub(super) async fn list_episodes(
        &self,
        input: p::ListRequest,
    ) -> Result<p::ListEpisodesResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let scope = format!("episodes:{}:{}", subject.0, input.status);
        let (limit, after) = page(input.page, &scope)?;
        let (items, more) = self
            .require_memory()?
            .list_episodes(subject, &input.status, limit, after)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .object
                    .episode_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListEpisodesResponse {
            items: items.into_iter().map(view).collect(),
            next_page_token,
        })
    }

    pub(super) async fn list_episode_revisions(
        &self,
        input: p::ListEpisodeRevisionsRequest,
    ) -> Result<p::ListEpisodeRevisionsResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let episode = EpisodeId(id(&input.episode_id)?);
        let scope = format!("episode-history:{}:{}", subject.0, episode.0);
        let (limit, after) = page(input.page, &scope)?;
        let (values, more) = self
            .require_memory()?
            .episode_history(subject, episode, limit, after)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                values
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .revision
                    .episode_revision_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListEpisodeRevisionsResponse {
            items: values
                .into_iter()
                .map(|value| revision(value.revision, value.members, value.basis, value.relations))
                .collect(),
            next_page_token,
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

                    content: episode_content(required(input.content, "content")?)?,
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
        action: nous_memory::LifecycleAction,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let episode = EpisodeId(id(&input.episode_id)?);
        let operation_id = OperationId(id(&input.operation_id)?);
        let value = self
            .require_memory()?
            .mutate_episode_lifecycle(
                subject,
                episode,
                operation_id,
                input.expected_object_epoch,
                action,
            )
            .await?;
        Ok(response(value))
    }

    pub(super) async fn suppress_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, nous_memory::LifecycleAction::Suppress)
            .await
    }
    pub(super) async fn restore_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, nous_memory::LifecycleAction::Restore)
            .await
    }
    pub(super) async fn withdraw_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, nous_memory::LifecycleAction::Withdraw)
            .await
    }
    pub(super) async fn reaccept_episode(
        &self,
        input: p::EpisodeMutationRequest,
    ) -> Result<p::EpisodeResponse> {
        self.episode_lifecycle(input, nous_memory::LifecycleAction::Reaccept)
            .await
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
