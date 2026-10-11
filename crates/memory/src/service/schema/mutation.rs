// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn create_schema(&self, mut input: CreateSchemaInput) -> Result<SchemaView> {
        input.content.producer = canonical_schema_producer(input.content.producer.as_ref())?;
        self.store.require_subject(input.subject).await?;
        let digest = operation_digest(
            "create_cognitive_schema",
            input.subject,
            &serde_json::json!({"title":input.content.title,"structural_claim":input.content.structural_claim,"scope":input.content.applicability_scope,"boundary_definition":input.content.boundary_definition,"formation_kind":input.content.formation_kind,"evidence_links":input.content.evidence_links,"producer":input.content.producer}),
        )?;
        let mut mutation = match self
            .start_mutation(
                input.subject,
                input.operation_id,
                "create_cognitive_schema",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                let id = receipt.result_ref;
                if receipt.state == "committed" {
                    return self
                        .schema_at(
                            input.subject,
                            CognitiveSchemaId(
                                id.ok_or_else(|| {
                                    Error::Infrastructure("schema receipt missing result".into())
                                })?
                                .parse()
                                .map_err(|_| {
                                    Error::Infrastructure("invalid schema receipt".into())
                                })?,
                            ),
                            receipt.result_revision.map(CognitiveSchemaRevisionId),
                        )
                        .await;
                }
                return Err(Error::Unavailable(
                    "schema operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        self.validate_schema_formation(input.subject, &input.content)
            .await?;
        let producer = if let Some(value) = &input.content.producer {
            Some(AuthorityStore::register_producer_in(mutation.tx(), value).await?)
        } else {
            None
        };
        let (schema_id, revision_id) = self
            .create_schema_in(
                mutation.tx(),
                &input,
                producer,
                self.cognition.now(input.subject),
            )
            .await?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::CognitiveSchemaRevision(revision_id),
            sequence,
        )
        .await?;
        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::CognitiveSchema(schema_id),
                    CognitiveRef::CognitiveSchemaRevision(revision_id),
                ],
            )
            .await?;
        mutation
            .commit(
                "schema",
                Some(&schema_id.0.to_string()),
                Some(revision_id.0),
                Some(1),
            )
            .await?;
        self.schema_revision(input.subject, revision_id).await
    }

    pub(in crate::service) async fn insert_schema_link(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        revision: CognitiveSchemaRevisionId,
        input: SchemaEvidenceLinkInput,
    ) -> Result<()> {
        let id = SchemaEvidenceLinkId::new();
        let epistemic_relation = input.basis.epistemic_relation();
        let (
            kind,
            value,
            basis_role,
            occurrence,
            source_region,
            derived_representation,
            derived_region,
        ) = match input.basis {
            RevisionBasis::Evidence(e) => {
                let (sr, dr, drr) = match e.locator {
                    EvidenceLocator::WholeOccurrence => (None, None, None),
                    EvidenceLocator::SourceRegion(id) => (Some(id.0), None, None),
                    EvidenceLocator::DerivedRepresentation(id) => (None, Some(id.0), None),
                    EvidenceLocator::DerivedRegion(id) => (None, None, Some(id.0)),
                };
                (
                    "evidence".to_owned(),
                    e.canonical_key(),
                    e.basis_role.as_str().to_owned(),
                    Some(e.occurrence_id.0),
                    sr,
                    dr,
                    drr,
                )
            }
            RevisionBasis::CognitionDependency(d) => {
                let (k, v) = reference_parts(&d.target_revision);
                (
                    k,
                    v,
                    d.basis_role.as_str().to_owned(),
                    None,
                    None,
                    None,
                    None,
                )
            }
            RevisionBasis::Seed(_) => {
                return Err(Error::Invalid(
                    "CognitiveSchema cannot use Cognitive Seed support".into(),
                ));
            }
        };
        sqlx::query("INSERT INTO cognitive_schema_evidence_links(link_id,subject_id,schema_revision_id,role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,created_at,epistemic_relation) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(id.0).bind(subject.0).bind(revision.0).bind(input.role.as_str()).bind(kind).bind(value).bind(basis_role).bind(occurrence).bind(source_region).bind(derived_representation).bind(derived_region).bind(self.cognition.now(subject)).bind(epistemic_relation_text(epistemic_relation)).execute(&mut **tx).await.map_err(db)?;
        Ok(())
    }

    pub async fn add_schema_evidence(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
        operation_id: OperationId,
        expected_object_epoch: i64,
        link: SchemaEvidenceLinkInput,
    ) -> Result<SchemaView> {
        self.store.require_subject(subject).await?;
        self.validate_basis_for_subject(subject, std::slice::from_ref(&link.basis))
            .await?;
        let digest = operation_digest(
            "add_schema_evidence",
            subject,
            &serde_json::json!({"schema_id":schema_id,"expected_object_epoch":expected_object_epoch,"link":link}),
        )?;
        let mut mutation = match self
            .start_mutation(subject, operation_id, "add_schema_evidence", &digest)
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self.schema(subject, schema_id).await;
                }
                return Err(Error::Unavailable(
                    "schema evidence operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row=sqlx::query("SELECT current_revision_id,object_epoch FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 FOR UPDATE").bind(subject.0).bind(schema_id.0).fetch_optional(&mut **mutation.tx()).await.map_err(db)?.ok_or_else(||Error::NotFound("CognitiveSchema not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        crate::service::state::fence_epoch(epoch, expected_object_epoch)?;
        self.validate_basis_in_tx(mutation.tx(), subject, std::slice::from_ref(&link.basis))
            .await?;
        self.insert_schema_link(
            mutation.tx(),
            subject,
            CognitiveSchemaRevisionId(row.try_get("current_revision_id").map_err(db)?),
            link,
        )
        .await?;
        sqlx::query("UPDATE cognitive_schemas SET object_epoch=object_epoch+1 WHERE subject_id=$1 AND schema_id=$2").bind(subject.0).bind(schema_id.0).execute(&mut **mutation.tx()).await.map_err(db)?;
        let sequence = mutation
            .invalidate(ProjectionInvalidation::topology())
            .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            subject,
            "schema",
            &[schema_id.0],
            sequence,
            "source_basis_changed",
        )
        .await?;
        mutation
            .commit(
                "schema",
                Some(&schema_id.0.to_string()),
                None,
                Some(epoch + 1),
            )
            .await?;
        self.schema(subject, schema_id).await
    }

    #[expect(
        clippy::too_many_lines,
        reason = "schema content revision owns one atomic Authority transaction"
    )]
    pub async fn revise_schema(&self, mut input: ReviseSchemaInput) -> Result<SchemaView> {
        self.store.require_subject(input.subject).await?;
        input.content.producer = canonical_schema_producer(input.content.producer.as_ref())?;
        validate_schema_revision_content(&input)?;
        let digest = operation_digest(
            "revise_cognitive_schema",
            input.subject,
            &serde_json::json!({
                "schema_id": input.schema_id,
                "expected_object_epoch": input.expected_object_epoch,
                "intent": input.intent,
                "title": input.content.title,
                "structural_claim": input.content.structural_claim,
                "applicability_scope": input.content.applicability_scope,
                "boundary_definition": input.content.boundary_definition,
                "copy_link_ids": input.copy_link_ids,
                "evidence_links": input.content.evidence_links,
                "producer": input.content.producer,
                "formation_kind": input.content.formation_kind,
            }),
        )?;
        let mut mutation = match self
            .start_mutation(
                input.subject,
                input.operation_id,
                "revise_cognitive_schema",
                &digest,
            )
            .await?
        {
            MutationStart::Replay(receipt) => {
                if receipt.state == "committed" {
                    return self
                        .schema_at(
                            input.subject,
                            input.schema_id,
                            receipt.result_revision.map(CognitiveSchemaRevisionId),
                        )
                        .await;
                }
                return Err(Error::Unavailable(
                    "schema revision operation is already in progress".into(),
                ));
            }
            MutationStart::Active(mutation) => mutation,
        };
        let row = sqlx::query(
            "SELECT current_revision_id,object_epoch FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2 FOR UPDATE",
        )
        .bind(input.subject.0)
        .bind(input.schema_id.0)
        .fetch_optional(&mut **mutation.tx())
        .await
        .map_err(db)?
        .ok_or_else(|| Error::NotFound("CognitiveSchema not found".into()))?;
        let epoch: i64 = row.try_get("object_epoch").map_err(db)?;
        crate::service::state::fence_epoch(epoch, input.expected_object_epoch)?;
        let parent = CognitiveSchemaRevisionId(row.try_get("current_revision_id").map_err(db)?);
        let revision_no: i32 = sqlx::query_scalar(
            "SELECT revision_no+1 FROM cognitive_schema_revisions WHERE schema_revision_id=$1",
        )
        .bind(parent.0)
        .fetch_one(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let mut copied_links = input.content.evidence_links.clone();
        for link_id in &input.copy_link_ids {
            let link = sqlx::query("SELECT schema_revision_id,role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM cognitive_schema_evidence_links WHERE link_id=$1 AND subject_id=$2 AND revoked_at IS NULL")
                .bind(link_id.0)
                .bind(input.subject.0)
                .fetch_optional(&mut **mutation.tx())
                .await
                .map_err(db)?
                .ok_or_else(|| Error::Invalid("schema copy link is not active in Subject".into()))?;
            let source_revision =
                CognitiveSchemaRevisionId(link.try_get("schema_revision_id").map_err(db)?);
            if source_revision != parent {
                return Err(Error::Invalid(
                    "schema copy link must belong to the current parent revision".into(),
                ));
            }
            copied_links.push(decode_schema_link_input(link)?);
        }
        let copied_basis = copied_links
            .iter()
            .map(|link| link.basis.clone())
            .collect::<Vec<_>>();
        self.validate_object_dependency_cycle(
            input.subject,
            &format!("schema:{}", input.schema_id.0),
            &copied_basis,
        )
        .await?;
        self.validate_basis_in_tx(mutation.tx(), input.subject, &copied_basis)
            .await?;
        let revision_id = CognitiveSchemaRevisionId::new();
        let parent_scope = sqlx::query(
            "SELECT formation_kind,aboutness FROM cognitive_schema_revisions WHERE schema_revision_id=$1",
        )
        .bind(parent.0)
        .fetch_one(&mut **mutation.tx())
        .await
        .map_err(db)?;
        let formation_kind: String = parent_scope.try_get("formation_kind").map_err(db)?;
        let parent_aboutness: Vec<String> = parent_scope.try_get("aboutness").map_err(db)?;
        if input.content.formation_kind.as_str() != formation_kind
            || (!parent_aboutness.is_empty()
                && !input
                    .content
                    .applicability_scope
                    .aboutness
                    .iter()
                    .any(|entity| {
                        parent_aboutness
                            .iter()
                            .any(|value| value == entity.as_str())
                    }))
        {
            return Err(Error::Invalid(
                "Schema revision must preserve formation kind and aboutness continuity".into(),
            ));
        }
        let content = SchemaContent {
            evidence_links: copied_links,
            ..input.content.clone()
        };
        self.validate_schema_formation(input.subject, &content)
            .await?;
        let producer = if let Some(value) = &input.content.producer {
            Some(AuthorityStore::register_producer_in(mutation.tx(), value).await?)
        } else {
            None
        };
        self.write_schema_revision_in(
            mutation.tx(),
            input.subject,
            &content,
            SchemaRevisionWrite {
                schema_id: input.schema_id,
                revision_id,
                parent: Some(parent),
                intent: Some(input.intent),
                number: revision_no,
                formed_at: self.cognition.now(input.subject),
                recorded_at: self.cognition.now(input.subject),
                producer,
            },
        )
        .await?;
        sqlx::query("UPDATE cognitive_schemas SET current_revision_id=$3,object_epoch=object_epoch+1,integrity_state='valid' WHERE subject_id=$1 AND schema_id=$2")
            .bind(input.subject.0)
            .bind(input.schema_id.0)
            .bind(revision_id.0)
            .execute(&mut **mutation.tx())
            .await
            .map_err(db)?;
        let sequence = mutation.invalidate(ProjectionInvalidation::all()).await?;
        self.enqueue_concept_in(
            mutation.tx(),
            input.subject,
            CognitiveRef::CognitiveSchemaRevision(revision_id),
            sequence,
        )
        .await?;
        self.invalidate_object_dependents_in(
            mutation.tx(),
            input.subject,
            "schema",
            &[input.schema_id.0],
            sequence,
            "source_revised",
        )
        .await?;
        mutation
            .publish_workflow_results(
                "memory",
                &[
                    CognitiveRef::CognitiveSchema(input.schema_id),
                    CognitiveRef::CognitiveSchemaRevision(revision_id),
                ],
            )
            .await?;
        mutation
            .commit(
                "schema_revision",
                Some(&input.schema_id.0.to_string()),
                Some(revision_id.0),
                Some(epoch + 1),
            )
            .await?;
        self.schema_revision(input.subject, revision_id).await
    }
}
