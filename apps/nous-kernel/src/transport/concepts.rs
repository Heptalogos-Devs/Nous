// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{CognitiveRef, OperationId, Result, SubjectId};
use nous_memory::MutationResult;
use nous_memory::{AssociationBasis, UseEventRef};
use nous_memory::{CreateAssociationRequest, CreateTagRequest};
use nous_persistence::database_error as db;
use sqlx::Row;

impl KernelService {
    pub(super) async fn create_tag(&self, input: p::CreateTagRequest) -> Result<p::Tag> {
        let subject = SubjectId(id(&input.subject_id)?);
        let tag = required(input.tag, "tag")?;
        let result = self
            .require_memory()?
            .create_tag(
                subject,
                CreateTagRequest {
                    producer: input.producer.map(from_producer).transpose()?,
                    operation_id: OperationId(id(&input.operation_id)?),
                    label: tag.label,
                    description: tag.description,
                    kind_hint: tag.kind_hint,
                    origin: tag.origin,
                },
            )
            .await?;
        self.read_mutation_tag(subject, result).await
    }
    pub(super) async fn get_tag(&self, input: p::ObjectRequest) -> Result<p::Tag> {
        let row=sqlx::query("SELECT t.tag_id,t.current_revision_id,t.status,t.canonical_tag_id,r.label,r.description,r.kind_hint,r.origin FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.tag_id=$2")
            .bind(id(&input.subject_id)?).bind(id(&input.id)?).fetch_optional(self.0.store.pool()).await.map_err(db)?.ok_or_else(||Error::new(nous_core::ErrorOwner::Concept, nous_core::ErrorCode::NotFound, "Tag not found"))?;
        tag_row(row)
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
        self.read_mutation_tag(SubjectId(id(&input.subject_id)?), result)
            .await
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
        self.read_mutation_tag(SubjectId(id(&input.subject_id)?), result)
            .await
    }
    pub(super) async fn split_tag(&self, input: p::SplitTagRequest) -> Result<p::SplitTagResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
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
        let mut children = Vec::new();
        for result in results {
            children.push(self.read_mutation_tag(subject, result).await?);
        }
        Ok(p::SplitTagResponse { children })
    }
    async fn read_mutation_tag(
        &self,
        subject: SubjectId,
        result: MutationResult,
    ) -> Result<p::Tag> {
        let CognitiveRef::Tag(tag) = result.reference else {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InternalFailure,
                "tag mutation result has a non-Tag reference",
            ));
        };
        match result.revision_id {
            Some(revision) => self.read_tag_revision(subject, tag, revision).await,
            None => {
                self.get_tag(p::ObjectRequest {
                    subject_id: subject.0.to_string(),
                    id: tag.0.to_string(),
                })
                .await
            }
        }
    }
    async fn read_tag_revision(
        &self,
        subject: SubjectId,
        tag: nous_core::TagId,
        revision: uuid::Uuid,
    ) -> Result<p::Tag> {
        let row = sqlx::query("SELECT t.tag_id,r.tag_revision_id current_revision_id,t.status,t.canonical_tag_id,r.label,r.description,r.kind_hint,r.origin FROM tags t JOIN tag_revisions r ON r.tag_id=t.tag_id WHERE t.subject_id=$1 AND t.tag_id=$2 AND r.tag_revision_id=$3")
            .bind(subject.0).bind(tag.0).bind(revision).fetch_one(self.0.store.pool()).await.map_err(db)?;
        tag_row(row)
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
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
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
        self.0.store.require_subject(subject).await?;
        let status = if status.is_empty() { "active" } else { status };
        if !matches!(status, "active" | "withdrawn") {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InvalidRequest,
                "invalid tag status",
            ));
        }
        let scope = format!("tags:{}:{status}:{text}", subject.0);
        let (limit, last) = page(request_page, &scope)?;
        let rows = sqlx::query("SELECT t.tag_id,t.current_revision_id,t.status,t.canonical_tag_id,r.label,r.description,r.kind_hint,r.origin FROM tags t JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.status=$2 AND ($3::uuid IS NULL OR t.tag_id>$3) AND ($4='' OR strpos(lower(r.label || ' ' || COALESCE(r.description,'')),lower($4))>0 OR EXISTS(SELECT 1 FROM lexical_bindings b JOIN lexical_visibility v ON v.lexical_ref=b.lexical_ref WHERE v.subject_id=t.subject_id AND b.object_kind='tag' AND canonical_tag(t.subject_id,CASE WHEN b.object_kind='tag' THEN b.canonical_ref::uuid END)=t.tag_id AND b.tombstoned_at IS NULL AND (strpos(lower(v.display_name),lower($4))>0 OR EXISTS(SELECT 1 FROM unnest(v.aliases) alias WHERE strpos(lower(alias),lower($4))>0)))) ORDER BY t.tag_id LIMIT $5")
            .bind(subject.0).bind(status).bind(last).bind(text).bind(limit+1).fetch_all(self.0.store.pool()).await.map_err(db)?;
        let truncated = rows.len() > usize::try_from(limit).unwrap_or(200);
        let mut items = Vec::new();
        for row in rows.into_iter().take(usize::try_from(limit).unwrap_or(200)) {
            items.push(tag_row(row)?);
        }
        let next_page_token = if truncated {
            next_token(
                &scope,
                id(&items
                    .last()
                    .ok_or_else(|| {
                        Error::new(
                            nous_core::ErrorOwner::Concept,
                            nous_core::ErrorCode::InternalFailure,
                            "empty tag page",
                        )
                    })?
                    .tag_id)?,
            )
        } else {
            String::new()
        };
        Ok(p::ListTagsResponse {
            items,
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
    match value.basis.ok_or_else(|| {
        Error::new(
            nous_core::ErrorOwner::Concept,
            nous_core::ErrorCode::InvalidRequest,
            "empty association support",
        )
    })? {
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

fn tag_row(row: sqlx::postgres::PgRow) -> Result<p::Tag> {
    Ok(p::Tag {
        tag_id: row
            .try_get::<uuid::Uuid, _>("tag_id")
            .map_err(db)?
            .to_string(),
        current_revision_id: row
            .try_get::<uuid::Uuid, _>("current_revision_id")
            .map_err(db)?
            .to_string(),
        status: row.try_get("status").map_err(db)?,
        canonical_tag_id: row
            .try_get::<Option<uuid::Uuid>, _>("canonical_tag_id")
            .map_err(db)?
            .map(|id| id.to_string()),
        label: row.try_get("label").map_err(db)?,
        description: row.try_get("description").map_err(db)?,
        kind_hint: row.try_get("kind_hint").map_err(db)?,
        origin: row.try_get("origin").map_err(db)?,
    })
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
