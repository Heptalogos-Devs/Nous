// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{EpisodeId, EpisodeRevisionId, OperationId, Result, SubjectId};
use nous_memory::{CognitionView, EpisodeInput, EpisodeMemberInput, ReviseEpisodeInput};

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

pub(super) fn revision_proto(
    value: nous_memory::EpisodeRevision,
    members: Vec<nous_memory::EpisodeMember>,
    basis: Vec<nous_memory::RevisionBasis>,
) -> p::EpisodeRevision {
    p::EpisodeRevision {
        header: Some(super::cognition::cognition_revision_header_proto(
            &value.header,
        )),
        revision_intent: value.revision_intent,
        title: value.title,
        parent_episode_revision_id: value.parent_episode_revision_id.map(|v| v.0.to_string()),
        experience_time: Some(temporal_proto(&value.experience_time)),
        boundary_explanation: value.boundary_explanation,
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
    p::Episode {
        header: Some(super::cognition::cognition_header_proto(&value.header)),
        details: Some(p::EpisodeDetails {
            track_key: value.header.details.track_key().to_owned(),
        }),
        revision: Some(revision_proto(value.revision, value.members, value.basis)),
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
    async fn episode_mutation_response(
        &self,
        subject: SubjectId,
        result: &nous_memory::MutationResult,
    ) -> Result<p::EpisodeResponse> {
        let mut views = self
            .require_memory()?
            .read_mutation_cognitions(subject, std::slice::from_ref(result))
            .await?;
        match views.pop() {
            Some(CognitionView::Episode(value)) => Ok(response(value)),
            _ => Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InternalFailure,
                "episode mutation result did not materialize as Episode",
            )),
        }
    }

    pub(super) async fn create_episode(
        &self,
        input: p::CreateEpisodeRequest,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let result = self
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
        self.episode_mutation_response(subject, &result).await
    }

    pub(super) async fn revise_episode(
        &self,
        input: p::ReviseEpisodeRequest,
    ) -> Result<p::EpisodeResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let result = self
            .require_memory()?
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
            .await?;
        self.episode_mutation_response(subject, &result).await
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
}
