// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
mod lifecycle;
mod lineage;
mod mutation;
mod read;

pub use lifecycle::SchemaLifecycleAction;

use nous_persistence::database_error as db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaView {
    pub schema: CognitiveSchema,
    pub revision: CognitiveSchemaRevision,
    pub evidence_links: Vec<SchemaEvidenceLink>,
}

fn validate_schema_content(content: &SchemaContent) -> Result<()> {
    if content.structural_claim.trim().is_empty()
        || content.structural_claim.len() > 16 * 1024
        || content.applicability_scope.description.trim().is_empty()
        || content.applicability_scope.description.len() > 16 * 1024
        || content.boundary_definition.trim().is_empty()
        || content.boundary_definition.len() > 16 * 1024
    {
        return Err(Error::Invalid(
            "schema content is required and bounded".into(),
        ));
    }
    content.applicability_scope.valid_time.validate()
}

fn validate_schema_revision_content(input: &ReviseSchemaInput) -> Result<()> {
    if input.operation_id.0.is_nil() {
        return Err(Error::Invalid("operation_id is required".into()));
    }
    validate_schema_content(&input.content)
}

async fn active_schema_link_inputs(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    revision: CognitiveSchemaRevisionId,
) -> Result<Vec<SchemaEvidenceLinkInput>> {
    let rows = sqlx::query("SELECT role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=$2 AND revoked_at IS NULL ORDER BY link_id")
        .bind(subject.0)
        .bind(revision.0)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    rows.into_iter().map(decode_schema_link_input).collect()
}

fn decode_schema_link_input(row: sqlx::postgres::PgRow) -> Result<SchemaEvidenceLinkInput> {
    let basis_kind: String = row.try_get("basis_kind").map_err(db)?;
    let basis_role = parse_enum(row.try_get("basis_role").map_err(db)?, "schema basis role")?;
    let basis = if basis_kind == "evidence" {
        let locator = match (
            row.try_get::<Option<Uuid>, _>("source_region_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_representation_id")
                .map_err(db)?,
            row.try_get::<Option<Uuid>, _>("derived_region_id")
                .map_err(db)?,
        ) {
            (Some(id), None, None) => EvidenceLocator::SourceRegion(nous_core::SourceRegionId(id)),
            (None, Some(id), None) => {
                EvidenceLocator::DerivedRepresentation(nous_core::DerivedRepresentationId(id))
            }
            (None, None, Some(id)) => {
                EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(id))
            }
            (None, None, None) => EvidenceLocator::WholeOccurrence,
            _ => {
                return Err(Error::Infrastructure(
                    "schema evidence locator is invalid".into(),
                ));
            }
        };
        RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
            locator,
            basis_role,
        })
    } else {
        RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            target_revision: parse_reference(
                &basis_kind,
                &row.try_get::<String, _>("basis_ref").map_err(db)?,
            )?,
            basis_role,
        })
    };
    Ok(SchemaEvidenceLinkInput {
        role: parse_enum(row.try_get("role").map_err(db)?, "schema evidence role")?,
        basis,
    })
}

pub(super) struct SchemaRevisionWrite {
    pub schema_id: CognitiveSchemaId,
    pub revision_id: CognitiveSchemaRevisionId,
    pub parent: Option<CognitiveSchemaRevisionId>,
    pub intent: Option<RevisionIntent>,
    pub number: i32,
    pub formed_at: DateTime<Utc>,
    pub recorded_at: DateTime<Utc>,
    pub producer: Option<Uuid>,
}
impl MemoryService {
    pub(super) async fn validate_schema_formation(
        &self,
        subject: SubjectId,
        content: &SchemaContent,
    ) -> Result<()> {
        validate_schema_content(content)?;
        let basis: Vec<_> = content
            .evidence_links
            .iter()
            .map(|link| link.basis.clone())
            .collect();
        if basis.is_empty() {
            return Err(Error::Invalid(
                "Schema revision requires evidence links".into(),
            ));
        }
        for basis in &basis {
            validate_exact_basis(std::slice::from_ref(basis))?;
        }
        self.validate_basis_for_subject(subject, &basis).await?;
        if content.formation_kind == SchemaFormationKind::Synthesized {
            self.validate_formation_semantics(subject, FormationMode::Synthesized, &basis)
                .await?;
        }
        Ok(())
    }
    pub(super) async fn create_schema_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        input: &CreateSchemaInput,
        producer: Option<Uuid>,
        formed: DateTime<Utc>,
    ) -> Result<(CognitiveSchemaId, CognitiveSchemaRevisionId)> {
        let schema_id = CognitiveSchemaId::new();
        let revision_id = CognitiveSchemaRevisionId::new();
        let now = self.cognition.now(input.subject);
        let basis: Vec<_> = input
            .content
            .evidence_links
            .iter()
            .map(|link| link.basis.clone())
            .collect();
        self.validate_basis_in_tx(tx, input.subject, &basis).await?;
        sqlx::query("INSERT INTO cognitive_schemas(schema_id,subject_id,current_revision_id,object_epoch,acceptance_state,integrity_state,suppression_state,purge_state,created_at) VALUES($1,$2,$3,1,'accepted','valid','normal','normal',$4)")
            .bind(schema_id.0).bind(input.subject.0).bind(revision_id.0).bind(now).execute(&mut **tx).await.map_err(db)?;
        self.write_schema_revision_in(
            tx,
            input.subject,
            &input.content,
            SchemaRevisionWrite {
                schema_id,
                revision_id,
                parent: None,
                intent: None,
                number: 1,
                formed_at: formed,
                recorded_at: now,
                producer,
            },
        )
        .await?;
        Ok((schema_id, revision_id))
    }
    pub(super) async fn write_schema_revision_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        content: &SchemaContent,
        write: SchemaRevisionWrite,
    ) -> Result<()> {
        let (kind, start, end) = temporal_columns(&content.applicability_scope.valid_time);
        let aboutness: Vec<String> = content
            .applicability_scope
            .aboutness
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect();
        let tags: Vec<Uuid> = content
            .applicability_scope
            .tags
            .iter()
            .map(|id| id.0)
            .collect();
        sqlx::query("INSERT INTO cognitive_schema_revisions(schema_revision_id,schema_id,revision_no,parent_revision_id,revision_intent,title,structural_claim,applicability_description,aboutness,tags,boundary_definition,formation_kind,valid_time_kind,valid_time_start,valid_time_end,formed_at,recorded_at,producer_signature_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)")
            .bind(write.revision_id.0).bind(write.schema_id.0).bind(write.number).bind(write.parent.map(|id|id.0)).bind(write.intent.map(|intent|intent.as_str())).bind(&content.title).bind(&content.structural_claim).bind(&content.applicability_scope.description).bind(aboutness).bind(tags).bind(&content.boundary_definition).bind(content.formation_kind.as_str()).bind(kind).bind(start).bind(end).bind(write.formed_at).bind(write.recorded_at).bind(write.producer).execute(&mut **tx).await.map_err(db)?;
        for link in &content.evidence_links {
            self.insert_schema_link(tx, subject, write.revision_id, link.clone())
                .await?;
        }
        self.store
            .ensure_identity_addresses_in(
                tx,
                subject,
                &[
                    CognitiveRef::CognitiveSchema(write.schema_id),
                    CognitiveRef::CognitiveSchemaRevision(write.revision_id),
                ],
                content.title.as_deref().unwrap_or(""),
            )
            .await?;
        Ok(())
    }
}

fn canonical_schema_producer(
    producer: Option<&ProducerSignature>,
) -> Result<Option<ProducerSignature>> {
    producer
        .map(|producer| {
            if producer.operation != CapabilityOperation::MemoryConsolidationText {
                return Err(Error::Invalid(
                    "Schema producer requires a cognition consolidation operation".into(),
                ));
            }
            AuthorityStore::canonical_producer(producer)
        })
        .transpose()
}
