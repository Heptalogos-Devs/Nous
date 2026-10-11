// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{CognitiveSchemaId, EntityRef, OperationId, Result, SubjectId, TagId};
use nous_memory::schema::{SchemaLifecycleAction, SchemaView};
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
    let scope = value
        .applicability_scope
        .ok_or_else(|| Error::Invalid("applicability_scope is required".into()))?;
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
                .map(|value| Ok(TagId(id(&value)?)))
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

fn schema_view(value: SchemaView) -> p::CognitiveSchema {
    let revision = &value.revision;
    p::CognitiveSchema {
        schema_id: value.schema.schema_id.0.to_string(),
        subject_id: value.schema.subject_id.0.to_string(),
        current_revision_id: value.schema.current_revision_id.0.to_string(),
        object_epoch: value.schema.object_epoch,
        acceptance_state: enum_name(value.schema.acceptance_state),
        integrity_state: enum_name(value.schema.integrity_state),
        suppression_state: enum_name(value.schema.suppression_state),
        purge_state: enum_name(value.schema.purge_state),
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
            .into_iter()
            .map(|link| p::SchemaEvidenceLink {
                link_id: link.link_id.0.to_string(),
                role: enum_name(link.role),
                basis: Some(basis_proto(link.basis)),
            })
            .collect(),
        formed_at: Some(timestamp(revision.formed_at)),
        recorded_at: Some(timestamp(revision.recorded_at)),
        formation_kind: enum_name(revision.formation_kind),
        producer_signature_id: revision.producer_signature_id.map(|id| id.to_string()),
    }
}

impl KernelService {
    async fn schema_lifecycle(
        &self,
        input: p::CognitiveSchemaMutationRequest,
        action: SchemaLifecycleAction,
    ) -> Result<p::CognitiveSchema> {
        Ok(schema_view(
            self.require_memory()?
                .mutate_schema_lifecycle(
                    SubjectId(id(&input.subject_id)?),
                    CognitiveSchemaId(id(&input.schema_id)?),
                    OperationId(id(&input.operation_id)?),
                    input.expected_object_epoch,
                    action,
                )
                .await?,
        ))
    }

    pub(super) async fn suppress_cognitive_schema(
        &self,
        input: p::CognitiveSchemaMutationRequest,
    ) -> Result<p::CognitiveSchema> {
        self.schema_lifecycle(input, SchemaLifecycleAction::Suppress)
            .await
    }
    pub(super) async fn restore_cognitive_schema(
        &self,
        input: p::CognitiveSchemaMutationRequest,
    ) -> Result<p::CognitiveSchema> {
        self.schema_lifecycle(input, SchemaLifecycleAction::Restore)
            .await
    }
    pub(super) async fn withdraw_cognitive_schema(
        &self,
        input: p::CognitiveSchemaMutationRequest,
    ) -> Result<p::CognitiveSchema> {
        self.schema_lifecycle(input, SchemaLifecycleAction::Withdraw)
            .await
    }
    pub(super) async fn reaccept_cognitive_schema(
        &self,
        input: p::CognitiveSchemaMutationRequest,
    ) -> Result<p::CognitiveSchema> {
        self.schema_lifecycle(input, SchemaLifecycleAction::Reaccept)
            .await
    }
    pub(super) async fn purge_cognitive_schema(
        &self,
        input: p::CognitiveSchemaMutationRequest,
    ) -> Result<()> {
        self.require_memory()?
            .purge_schema(
                SubjectId(id(&input.subject_id)?),
                CognitiveSchemaId(id(&input.schema_id)?),
                OperationId(id(&input.operation_id)?),
                input.expected_object_epoch,
            )
            .await
    }

    pub(super) async fn create_cognitive_schema(
        &self,
        input: p::CreateCognitiveSchemaRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let schema = required(input.schema, "schema")?;
        Ok(schema_view(
            self.require_memory()?
                .create_schema(schema_input(
                    subject,
                    OperationId(id(&input.operation_id)?),
                    schema,
                )?)
                .await?,
        ))
    }

    pub(super) async fn get_cognitive_schema(
        &self,
        input: p::GetCognitiveSchemaRequest,
    ) -> Result<p::CognitiveSchema> {
        Ok(schema_view(
            self.require_memory()?
                .schema(
                    SubjectId(id(&input.subject_id)?),
                    CognitiveSchemaId(id(&input.schema_id)?),
                )
                .await?,
        ))
    }

    pub(super) async fn get_cognitive_schema_revision(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::CognitiveSchema> {
        Ok(schema_view(
            self.require_memory()?
                .schema_revision(
                    SubjectId(id(&input.subject_id)?),
                    nous_core::CognitiveSchemaRevisionId(id(&input.id)?),
                )
                .await?,
        ))
    }

    pub(super) async fn add_schema_evidence(
        &self,
        input: p::AddSchemaEvidenceRequest,
    ) -> Result<p::CognitiveSchema> {
        let subject = SubjectId(id(&input.subject_id)?);
        let link = required(input.link, "link")?;
        let basis = basis(required(link.basis, "basis")?)?;
        Ok(schema_view(
            self.require_memory()?
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
                .await?,
        ))
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
        Ok(schema_view(
            self.require_memory()?.revise_schema(value).await?,
        ))
    }

    pub(super) async fn split_cognitive_schema(
        &self,
        input: p::SplitCognitiveSchemaRequest,
    ) -> Result<p::SplitCognitiveSchemaResponse> {
        let subject = SubjectId(id(&input.subject_id)?);
        let operation = OperationId(id(&input.operation_id)?);
        let children = input
            .children
            .into_iter()
            .map(|child| schema_input(subject, operation, child))
            .collect::<Result<Vec<_>>>()?;
        let values = self
            .require_memory()?
            .split_schemas(
                subject,
                CognitiveSchemaId(id(&input.schema_id)?),
                operation,
                input.expected_object_epoch,
                children,
            )
            .await?;
        Ok(p::SplitCognitiveSchemaResponse {
            children: values.into_iter().map(schema_view).collect(),
        })
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
        Ok(schema_view(
            self.require_memory()?
                .merge_schemas(
                    subject,
                    operation,
                    sources,
                    input.expected_object_epochs,
                    merged,
                )
                .await?,
        ))
    }
}
