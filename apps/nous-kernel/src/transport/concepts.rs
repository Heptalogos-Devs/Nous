// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::UseEventRef;
use nous_core::{OperationId, Result, SubjectId};
use nous_memory::AssociationBasis;
use nous_memory::{CreateAssociationRequest, CreateTagRequest};

impl KernelService {
    pub(super) async fn create_tag(&self, input: p::CreateTagRequest) -> Result<p::Tag> {
        let subject = SubjectId(id(&input.subject_id)?);
        let content = tag_content(required(input.content, "content")?);
        let created = self
            .require_memory()?
            .create_tag(
                subject,
                CreateTagRequest {
                    producer: input.producer.map(from_producer).transpose()?,
                    operation_id: OperationId(id(&input.operation_id)?),
                    content,
                    origin: input.origin,
                },
            )
            .await?;
        Ok(tag_view(created))
    }
    pub(super) async fn get_tag(&self, input: p::ObjectRequest) -> Result<p::Tag> {
        Ok(tag_view(
            self.require_memory()?
                .tag_view(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::TagId(id(&input.id)?),
                    None,
                )
                .await?,
        ))
    }
    pub(super) async fn revise_tag(&self, input: p::ReviseTagRequest) -> Result<p::Tag> {
        let result = self
            .require_memory()?
            .revise_tag(
                SubjectId(id(&input.subject_id)?),
                nous_memory::ReviseTagInput {
                    operation_id: OperationId(id(&input.operation_id)?),
                    target: tag_target(required(input.target, "target")?)?,
                    content: tag_content(required(input.content, "content")?),
                    producer: input.producer.map(from_producer).transpose()?,
                },
            )
            .await?;
        Ok(tag_view(result))
    }
    pub(super) async fn merge_tags(&self, input: p::MergeTagsRequest) -> Result<p::Tag> {
        let result = self
            .require_memory()?
            .merge_tags(
                SubjectId(id(&input.subject_id)?),
                nous_memory::MergeTagsInput {
                    operation_id: OperationId(id(&input.operation_id)?),
                    survivor: tag_target(required(input.survivor, "survivor")?)?,
                    retired: input
                        .retired
                        .into_iter()
                        .map(tag_target)
                        .collect::<Result<Vec<_>>>()?,
                    basis: input
                        .basis
                        .into_iter()
                        .map(super::basis)
                        .collect::<Result<Vec<_>>>()?,
                },
            )
            .await?;
        Ok(tag_view(result))
    }
    pub(super) async fn split_tag(&self, input: p::SplitTagRequest) -> Result<p::SplitTagResponse> {
        let results = self
            .require_memory()?
            .split_tag(
                SubjectId(id(&input.subject_id)?),
                nous_memory::SplitTagInput {
                    producer: input.producer.map(from_producer).transpose()?,
                    operation_id: OperationId(id(&input.operation_id)?),
                    parent: tag_target(required(input.parent, "parent")?)?,
                    children: input.children.into_iter().map(tag_content).collect(),
                    basis: input
                        .basis
                        .into_iter()
                        .map(super::basis)
                        .collect::<Result<Vec<_>>>()?,
                },
            )
            .await?;
        let children = results.into_iter().map(tag_view).collect();
        Ok(p::SplitTagResponse { children })
    }
    pub(super) async fn list_tags(&self, input: p::ListRequest) -> Result<p::ListTagsResponse> {
        self.read_tags(&input.subject_id, "", input.page, &input.status)
            .await
    }
    pub(super) async fn search_tags(
        &self,
        input: p::SearchTagsRequest,
    ) -> Result<p::ListTagsResponse> {
        if input.text.trim().is_empty() || input.text.chars().count() > 256 {
            return Err(Error::Invalid(
                "tag search requires 1..256 characters".into(),
            ));
        }
        self.read_tags(&input.subject_id, &input.text, input.page, "active")
            .await
    }
    async fn read_tags(
        &self,
        subject: &str,
        text: &str,
        request_page: Option<p::Page>,
        status: &str,
    ) -> Result<p::ListTagsResponse> {
        let subject = SubjectId(id(subject)?);
        let status = if status.is_empty() { "active" } else { status };
        let scope = format!("tags:{}:{status}:{text}", subject.0);
        let (limit, last) = page(request_page, &scope)?;
        let (items, more) = self
            .require_memory()?
            .tag_views(subject, text, status, limit, last)
            .await?;
        let next_page_token = if more {
            next_token(
                &scope,
                items
                    .last()
                    .ok_or_else(|| Error::Internal("empty page".into()))?
                    .tag_id
                    .0,
            )
        } else {
            String::new()
        };
        Ok(p::ListTagsResponse {
            items: items.into_iter().map(tag_view).collect(),
            next_page_token,
        })
    }
    pub(super) async fn create_association(
        &self,
        input: p::CreateAssociationRequest,
    ) -> Result<p::Association> {
        let subject = SubjectId(id(&input.subject_id)?);
        let association = required(input.association, "association")?;
        let from = from_ref(required(association.from, "association.from")?)?;
        let to = from_ref(required(association.to, "association.to")?)?;
        let basis = association
            .basis
            .into_iter()
            .map(association_basis)
            .collect::<Result<Vec<AssociationBasis>>>()?;
        let result = self
            .require_memory()?
            .create_association(
                CreateAssociationRequest {
                    producer: input.producer.map(from_producer).transpose()?,
                    operation_id: OperationId(id(&input.operation_id)?),
                    from: from.clone(),
                    to: to.clone(),
                    relation_kind: association.relation_kind.clone(),
                    polarity: enum_value(&association.polarity)?,
                    basis_class: enum_value(&association.basis_class)?,
                    basis,
                    producer_signature_id: association
                        .producer_signature_id
                        .as_deref()
                        .map(id)
                        .transpose()?,
                    valid_time: nous_core::TemporalExtent::Unknown,
                },
                subject,
            )
            .await?;
        Ok(p::Association {
            association_id: result.association_evidence_id.0.to_string(),
            from: Some(to_ref(result.from)),
            to: Some(to_ref(result.to)),
            relation_kind: result.relation_kind,
            polarity: enum_name(result.polarity),
            basis_class: enum_name(result.basis_class),
            basis: result
                .basis
                .into_iter()
                .map(association_basis_proto)
                .collect(),
            producer_signature_id: result.producer_signature_id.map(|value| value.to_string()),
        })
    }

    pub(super) async fn revoke_association(
        &self,
        input: p::RevokeAssociationRequest,
    ) -> Result<()> {
        self.require_memory()?
            .revoke_association(
                SubjectId(id(&input.subject_id)?),
                nous_core::AssociationEvidenceId(id(&input.association_id)?),
                OperationId(id(&input.operation_id)?),
            )
            .await
    }
    pub(super) async fn get_neighborhood(
        &self,
        input: p::NeighborhoodRequest,
    ) -> Result<p::NeighborhoodResponse> {
        let result = self
            .require_memory()?
            .association_neighborhood(
                SubjectId(id(&input.subject_id)?),
                from_ref(required(input.root, "root")?)?,
                if input.max_nodes == 0 {
                    64
                } else {
                    input.max_nodes as usize
                },
                if input.max_depth == 0 {
                    1
                } else {
                    input.max_depth as usize
                },
            )
            .await?;
        Ok(p::NeighborhoodResponse {
            nodes: result.nodes.into_iter().map(to_ref).collect(),
            associations: result
                .associations
                .into_iter()
                .map(|a| p::Association {
                    association_id: a.association_evidence_id.0.to_string(),
                    from: Some(to_ref(a.from)),
                    to: Some(to_ref(a.to)),
                    relation_kind: a.relation_kind,
                    polarity: enum_name(a.polarity),
                    basis_class: enum_name(a.basis_class),
                    basis: a.basis.into_iter().map(association_basis_proto).collect(),
                    producer_signature_id: a.producer_signature_id.map(|id| id.to_string()),
                })
                .collect(),
            truncated: result.truncated,
        })
    }
}

fn association_basis(value: p::AssociationBasis) -> Result<AssociationBasis> {
    match value
        .basis
        .ok_or_else(|| Error::Invalid("empty association support".into()))?
    {
        p::association_basis::Basis::Revision(value) => {
            Ok(AssociationBasis::Revision(super::basis(value)?))
        }
        p::association_basis::Basis::UseEvent(value) => {
            Ok(AssociationBasis::UseEvent(UseEventRef {
                subject_id: SubjectId(id(&value.subject_id)?),
                consumer_ref: value.consumer_ref,
                event_id: nous_core::UseEventId(id(&value.event_id)?),
            }))
        }
    }
}

pub(super) fn association_basis_proto(value: AssociationBasis) -> p::AssociationBasis {
    let basis = match value {
        AssociationBasis::Revision(value) => {
            p::association_basis::Basis::Revision(super::basis_proto(value))
        }
        AssociationBasis::UseEvent(value) => {
            p::association_basis::Basis::UseEvent(p::UseEventRef {
                subject_id: value.subject_id.0.to_string(),
                consumer_ref: value.consumer_ref,
                event_id: value.event_id.0.to_string(),
            })
        }
    };
    p::AssociationBasis { basis: Some(basis) }
}

fn tag_view(value: nous_memory::TagView) -> p::Tag {
    p::Tag {
        tag_id: value.tag_id.0.to_string(),
        current_revision_id: value.current_revision_id.to_string(),
        revision_id: value.revision_id.to_string(),
        status: value.status,
        canonical_tag_id: value.canonical_tag_id.map(|id| id.0.to_string()),
        label: value.label,
        description: value.description,
        kind_hint: value.kind_hint,
        origin: value.origin,
    }
}
fn tag_target(target: p::TagRevisionTarget) -> Result<nous_memory::TagExpectation> {
    Ok(nous_memory::TagExpectation {
        tag_id: nous_core::TagId(id(&target.tag_id)?),
        expected_revision_id: id(&target.expected_revision_id)?,
    })
}
fn tag_content(content: p::TagContent) -> nous_memory::TagContent {
    nous_memory::TagContent {
        label: content.label,
        description: content.description,
        kind_hint: content.kind_hint,
    }
}
