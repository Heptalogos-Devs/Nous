// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{CognitiveSchemaId, EntityRef, OperationId, Result, SubjectId, TagId};
use nous_memory::CognitionView;
use nous_memory::schema::SchemaView;
use nous_memory::{
    CreateSchemaInput, ReviseSchemaInput, SchemaContent, SchemaEvidenceLinkInput,
    SchemaFormationKind, SchemaScope,
};

fn schema_input(
    subject: SubjectId,
    operation_id: OperationId,
    value: p::CognitiveSchemaContent,
) -> Result<CreateSchemaInput> {
    Ok(CreateSchemaInput {
        subject,
        operation_id,
        content: schema_content(value)?,
    })
}

fn schema_content(value: p::CognitiveSchemaContent) -> Result<SchemaContent> {
    let scope = value.applicability_scope.ok_or_else(|| {
        Error::new(
            nous_core::ErrorOwner::Concept,
            nous_core::ErrorCode::InvalidRequest,
            "applicability_scope is required",
        )
    })?;
    let formation_kind = if value.formation_kind.is_empty() {
        SchemaFormationKind::ExplicitImport
    } else {
        enum_value(&value.formation_kind)?
    };
    Ok(SchemaContent {
        producer: value.producer.map(from_producer).transpose()?,
        title: (!value.title.is_empty()).then_some(value.title),
        structural_claim: value.structural_claim,
        applicability_scope: SchemaScope {
            description: scope.description,
            aboutness: scope
                .aboutness
                .into_iter()
                .map(EntityRef::new)
                .collect::<Result<_>>()?,
            tags: scope
                .tags
                .into_iter()
                .map(|value| id(value.as_str()).map(TagId))
                .collect::<Result<_>>()?,
            valid_time: temporal(scope.valid_time)?,
        },
        boundary_definition: value.boundary_definition,
        formation_kind,
        evidence_links: value
            .evidence_links
            .into_iter()
            .map(|link| {
                Ok(SchemaEvidenceLinkInput {
                    role: enum_value(&link.role)?,
                    basis: basis(required(link.basis, "basis")?)?,
                })
            })
            .collect::<Result<_>>()?,
    })
}

pub(super) fn schema_revision_proto(value: &SchemaView) -> p::CognitiveSchemaRevision {
    let revision = &value.revision;
    p::CognitiveSchemaRevision {
        header: Some(super::cognition::cognition_revision_header_proto(
            &revision.header,
        )),
        revision_intent: revision.revision_intent.map(enum_name),
        details: Some(p::CognitiveSchemaRevisionDetails {
            title: revision.title.clone().unwrap_or_default(),
            structural_claim: revision.structural_claim.clone(),
            applicability_scope: Some(p::SchemaScope {
                description: revision.applicability_scope.description.clone(),
                aboutness: revision
                    .applicability_scope
                    .aboutness
                    .iter()
                    .map(|value| value.as_str().to_owned())
                    .collect(),
                tags: revision
                    .applicability_scope
                    .tags
                    .iter()
                    .map(|value| value.0.to_string())
                    .collect(),
                valid_time: Some(temporal_proto(&revision.applicability_scope.valid_time)),
            }),
            boundary_definition: revision.boundary_definition.clone(),
            evidence_links: value
                .evidence_links
                .iter()
                .cloned()
                .map(|link| p::SchemaEvidenceLink {
                    link_id: link.link_id.0.to_string(),
                    role: enum_name(link.role),
                    basis: Some(basis_proto(link.basis)),
                })
                .collect(),
            formation_kind: enum_name(revision.formation_kind),
        }),
    }
}

pub(super) fn schema_view(value: SchemaView) -> p::CognitiveSchema {
    p::CognitiveSchema {
        header: Some(super::cognition::cognition_header_proto(&value.header)),
        revision: Some(schema_revision_proto(&value)),
    }
}

impl KernelService {
    async fn schema_mutation_response(
        &self,
        subject: SubjectId,
        result: &nous_memory::MutationResult,
    ) -> Result<p::CognitiveSchema> {
        let mut views = self
            .require_memory()?
            .read_mutation_cognitions(subject, std::slice::from_ref(result))
            .await?;
        match views.pop() {
            Some(CognitionView::CognitiveSchema(value)) => Ok(schema_view(value)),
            _ => Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InternalFailure,
                "schema mutation result did not materialize as CognitiveSchema",
            )),
        }
    }

    pub(super) async fn create_cognitive_schema(
        &self,
        input: p::CreateCognitiveSchemaRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let schema = required(input.schema, "schema")?;
        let result = self
            .require_memory()?
            .create_schema(schema_input(
                subject,
                OperationId(id(&input.operation_id)?),
                schema,
            )?)
            .await?;
        self.schema_mutation_response(subject, &result).await
    }

    pub(super) async fn add_schema_evidence(
        &self,
        input: p::AddSchemaEvidenceRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let link = required(input.link, "link")?;
        let basis = basis(required(link.basis, "basis")?)?;
        let result = self
            .require_memory()?
            .add_schema_evidence(
                subject,
                CognitiveSchemaId(id(&input.schema_id)?),
                OperationId(id(&input.operation_id)?),
                input.expected_object_epoch,
                SchemaEvidenceLinkInput {
                    role: enum_value(&link.role)?,
                    basis,
                },
            )
            .await?;
        self.schema_mutation_response(subject, &result).await
    }

    pub(super) async fn revise_cognitive_schema(
        &self,
        input: p::ReviseCognitiveSchemaRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let schema = required(input.schema, "schema")?;
        let content = schema_content(schema)?;
        let copy_link_ids = input
            .copy_link_ids
            .iter()
            .map(|value| Ok(nous_core::SchemaEvidenceLinkId(id(value)?)))
            .collect::<Result<Vec<_>>>()?;
        let value = ReviseSchemaInput {
            content,
            operation_id: OperationId(id(&input.operation_id)?),
            subject,
            schema_id: CognitiveSchemaId(id(&input.schema_id)?),
            expected_object_epoch: input.expected_object_epoch,
            intent: enum_value(&input.intent)?,
            copy_link_ids,
        };
        let result = self.require_memory()?.revise_schema(value).await?;
        self.schema_mutation_response(subject, &result).await
    }

    pub(super) async fn split_cognitive_schema(
        &self,
        input: p::SplitCognitiveSchemaRequest,
    ) -> Result<p::SplitCognitiveSchemaResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let operation = OperationId(id(&input.operation_id)?);
        let child_count = input.children.len();
        let children = input
            .children
            .into_iter()
            .map(|child| schema_input(subject, operation, child))
            .collect::<Result<Vec<_>>>()?;
        let memory = self.require_memory()?;
        let results = memory
            .split_schemas(
                subject,
                CognitiveSchemaId(id(&input.schema_id)?),
                operation,
                input.expected_object_epoch,
                children,
            )
            .await?;
        let child_results = results
            .into_iter()
            .filter(|result| {
                matches!(
                    &result.reference,
                    nous_core::CognitiveRef::CognitiveSchemaRevision(_)
                )
            })
            .collect::<Vec<_>>();
        if child_results.len() != child_count {
            return Err(Error::new(
                nous_core::ErrorOwner::Concept,
                nous_core::ErrorCode::InternalFailure,
                "schema split result count did not match its children",
            ));
        }
        let values = memory
            .read_mutation_cognitions(subject, &child_results)
            .await?;
        let children = values
            .into_iter()
            .map(|value| match value {
                CognitionView::CognitiveSchema(value) => Ok(schema_view(value)),
                _ => Err(Error::new(
                    nous_core::ErrorOwner::Concept,
                    nous_core::ErrorCode::InternalFailure,
                    "schema split result did not materialize as CognitiveSchema",
                )),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(p::SplitCognitiveSchemaResponse { children })
    }

    pub(super) async fn merge_cognitive_schemas(
        &self,
        input: p::MergeCognitiveSchemasRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let operation = OperationId(id(&input.operation_id)?);
        let sources = input
            .schema_ids
            .iter()
            .map(|value| Ok(CognitiveSchemaId(id(value)?)))
            .collect::<Result<Vec<_>>>()?;
        let merged_value = required(input.merged, "merged")?;
        let merged = schema_input(subject, operation, merged_value)?;
        let memory = self.require_memory()?;
        let result = memory
            .merge_schemas(
                subject,
                operation,
                sources,
                input.expected_object_epochs,
                merged,
            )
            .await?;
        self.schema_mutation_response(subject, &result).await
    }
}
