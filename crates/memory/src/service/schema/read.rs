// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashMap;

impl MemoryService {
    pub async fn schema(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
    ) -> Result<SchemaView> {
        self.schema_at(subject, schema_id, None).await
    }
    pub async fn schema_revision(
        &self,
        subject: SubjectId,
        revision: CognitiveSchemaRevisionId,
    ) -> Result<SchemaView> {
        let reference = CognitiveRef::CognitiveSchemaRevision(revision);
        self.schemas_by_references(subject, std::slice::from_ref(&reference), None)
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(
                    ErrorOwner::Concept,
                    ErrorCode::NotFound,
                    "CognitiveSchema revision not found",
                )
            })
    }
    pub(super) async fn schema_at(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
        revision: Option<CognitiveSchemaRevisionId>,
    ) -> Result<SchemaView> {
        let reference = revision.map_or(
            CognitiveRef::CognitiveSchema(schema_id),
            CognitiveRef::CognitiveSchemaRevision,
        );
        let view = self
            .schemas_by_references(subject, std::slice::from_ref(&reference), None)
            .await?
            .remove(&reference)
            .ok_or_else(|| {
                Error::new(
                    ErrorOwner::Concept,
                    ErrorCode::NotFound,
                    "CognitiveSchema not found",
                )
            })?;
        if view.schema.schema_id != schema_id {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::NotFound,
                "CognitiveSchema not found",
            ));
        }
        Ok(view)
    }

    /// Load complete CognitiveSchema views for the requested current objects
    /// and exact revisions in a single owner snapshot. Evidence links are
    /// grouped by revision, and historical link visibility is taken from the
    /// already-frozen Authority view when one is present.
    pub async fn schemas_by_references(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, SchemaView>> {
        if references.len() > 256 {
            return Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::InvalidRequest,
                "CognitiveSchema context batch exceeds 256 references",
            ));
        }
        let schema_ids = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::CognitiveSchema(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        let requested_revisions = references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        if schema_ids.is_empty() && requested_revisions.is_empty() {
            return Ok(HashMap::new());
        }
        let mut tx = self.store.pool().begin().await.map_err(db)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let rows = sqlx::query("SELECT s.schema_id,s.subject_id,s.current_revision_id,s.object_epoch,s.acceptance_state,s.integrity_state,s.suppression_state,s.purge_state,s.created_at,r.schema_revision_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.structural_claim,r.applicability_description,r.aboutness,r.tags,r.boundary_definition,r.formation_kind,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM cognitive_schemas s JOIN cognitive_schema_revisions r ON r.schema_id=s.schema_id AND ((s.schema_id=ANY($2) AND r.schema_revision_id=s.current_revision_id) OR r.schema_revision_id=ANY($3)) WHERE s.subject_id=$1 ORDER BY s.schema_id,r.revision_no")
            .bind(subject.0)
            .bind(&schema_ids)
            .bind(&requested_revisions)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let revision_ids = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("schema_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        if revision_ids.is_empty() {
            tx.commit().await.map_err(db)?;
            return Ok(HashMap::new());
        }
        let historical_link_ids = historical
            .map(|snapshot| snapshot.schema_evidence_links.clone())
            .unwrap_or_default();
        let link_rows = sqlx::query("SELECT link_id,subject_id,schema_revision_id,role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,producer_signature_id,created_at,revoked_at,epistemic_relation FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=ANY($2) AND (($3 AND link_id=ANY($4::uuid[])) OR (NOT $3 AND revoked_at IS NULL)) ORDER BY schema_revision_id,link_id")
            .bind(subject.0)
            .bind(&revision_ids)
            .bind(historical.is_some())
            .bind(&historical_link_ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let mut links = HashMap::<Uuid, Vec<SchemaEvidenceLink>>::new();
        for row in link_rows {
            let revision =
                CognitiveSchemaRevisionId(row.try_get("schema_revision_id").map_err(db)?);
            links
                .entry(revision.0)
                .or_default()
                .push(decode_schema_link(row, revision)?);
        }
        tx.commit().await.map_err(db)?;

        let mut by_schema = HashMap::<CognitiveSchemaId, SchemaView>::new();
        let mut by_revision = HashMap::<CognitiveSchemaRevisionId, SchemaView>::new();
        for row in rows {
            let schema_id = CognitiveSchemaId(row.try_get("schema_id").map_err(db)?);
            let revision_id =
                CognitiveSchemaRevisionId(row.try_get("schema_revision_id").map_err(db)?);
            let current_revision_id =
                CognitiveSchemaRevisionId(row.try_get("current_revision_id").map_err(db)?);
            let revision_intent = row
                .try_get::<Option<String>, _>("revision_intent")
                .map_err(db)?
                .map(|value| parse_enum(ErrorOwner::Concept, value, "schema revision intent"))
                .transpose()?;
            let revision = CognitiveSchemaRevision {
                schema_revision_id: revision_id,
                schema_id,
                revision_no: row.try_get("revision_no").map_err(db)?,
                parent_revision_id: row
                    .try_get::<Option<Uuid>, _>("parent_revision_id")
                    .map_err(db)?
                    .map(CognitiveSchemaRevisionId),
                revision_intent,
                title: row.try_get("title").map_err(db)?,
                structural_claim: row.try_get("structural_claim").map_err(db)?,
                applicability_scope: SchemaScope {
                    description: row.try_get("applicability_description").map_err(db)?,
                    aboutness: row
                        .try_get::<Vec<String>, _>("aboutness")
                        .map_err(db)?
                        .into_iter()
                        .map(EntityRef::new)
                        .collect::<Result<Vec<_>>>()?,
                    tags: row
                        .try_get::<Vec<Uuid>, _>("tags")
                        .map_err(db)?
                        .into_iter()
                        .map(TagId)
                        .collect(),
                    valid_time: temporal_from_columns(
                        ErrorOwner::Concept,
                        row.try_get("valid_time_kind").map_err(db)?,
                        row.try_get("valid_time_start").map_err(db)?,
                        row.try_get("valid_time_end").map_err(db)?,
                    )?,
                },
                boundary_definition: row.try_get("boundary_definition").map_err(db)?,
                formation_kind: parse_enum(
                    ErrorOwner::Concept,
                    row.try_get("formation_kind").map_err(db)?,
                    "schema formation kind",
                )?,
                formed_at: row.try_get("formed_at").map_err(db)?,
                recorded_at: row.try_get("recorded_at").map_err(db)?,
                producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
            };
            let view = SchemaView {
                schema: CognitiveSchema {
                    schema_id,
                    subject_id: subject,
                    current_revision_id,
                    object_epoch: row.try_get("object_epoch").map_err(db)?,
                    acceptance_state: parse_enum(
                        ErrorOwner::Concept,
                        row.try_get("acceptance_state").map_err(db)?,
                        "schema acceptance state",
                    )?,
                    integrity_state: parse_enum(
                        ErrorOwner::Concept,
                        row.try_get("integrity_state").map_err(db)?,
                        "schema integrity state",
                    )?,
                    suppression_state: parse_enum(
                        ErrorOwner::Concept,
                        row.try_get("suppression_state").map_err(db)?,
                        "schema suppression state",
                    )?,
                    purge_state: parse_enum(
                        ErrorOwner::Concept,
                        row.try_get("purge_state").map_err(db)?,
                        "schema purge state",
                    )?,
                    created_at: row.try_get("created_at").map_err(db)?,
                },
                revision,
                evidence_links: links.remove(&revision_id.0).unwrap_or_default(),
            };
            if view.schema.current_revision_id == revision_id {
                by_schema.insert(schema_id, view.clone());
            }
            by_revision.insert(revision_id, view);
        }
        let mut result = HashMap::new();
        for reference in references {
            let view = match reference {
                CognitiveRef::CognitiveSchema(id) => by_schema.get(id).cloned(),
                CognitiveRef::CognitiveSchemaRevision(id) => by_revision.get(id).cloned(),
                _ => None,
            };
            if let Some(view) = view {
                result.insert(reference.clone(), view);
            }
        }
        Ok(result)
    }

    pub(crate) async fn schema_links(
        &self,
        subject: SubjectId,
        revision: CognitiveSchemaRevisionId,
    ) -> Result<Vec<SchemaEvidenceLink>> {
        self.schema_links_in_view(subject, revision, None).await
    }

    pub(in crate::service) async fn schema_links_in_view(
        &self,
        subject: SubjectId,
        revision: CognitiveSchemaRevisionId,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<SchemaEvidenceLink>> {
        let rows=sqlx::query("SELECT link_id,subject_id,schema_revision_id,role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,producer_signature_id,created_at,revoked_at,epistemic_relation FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=$2 AND (($3 AND link_id=ANY($4::uuid[])) OR (NOT $3 AND revoked_at IS NULL)) ORDER BY link_id").bind(subject.0).bind(revision.0).bind(view.is_some()).bind(view.map(|view|view.schema_evidence_links.clone()).unwrap_or_default()).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| decode_schema_link(row, revision))
            .collect()
    }
}

fn decode_schema_link(
    row: sqlx::postgres::PgRow,
    revision: CognitiveSchemaRevisionId,
) -> Result<SchemaEvidenceLink> {
    let basis = if row.try_get::<String, _>("basis_kind").map_err(db)? == "evidence" {
        RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            occurrence_id: OccurrenceId(row.try_get("occurrence_id").map_err(db)?),
            locator: match (
                row.try_get::<Option<Uuid>, _>("source_region_id")
                    .map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_representation_id")
                    .map_err(db)?,
                row.try_get::<Option<Uuid>, _>("derived_region_id")
                    .map_err(db)?,
            ) {
                (Some(id), None, None) => {
                    EvidenceLocator::SourceRegion(nous_core::SourceRegionId(id))
                }
                (None, Some(id), None) => {
                    EvidenceLocator::DerivedRepresentation(nous_core::DerivedRepresentationId(id))
                }
                (None, None, Some(id)) => {
                    EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(id))
                }
                (None, None, None) => EvidenceLocator::WholeOccurrence,
                _ => {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Concept,
                        nous_core::ErrorCode::InternalFailure,
                        "schema evidence locator is invalid",
                    ));
                }
            },
            basis_role: parse_enum(
                ErrorOwner::Concept,
                row.try_get("basis_role").map_err(db)?,
                "schema evidence basis role",
            )?,
        })
    } else {
        RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation: parse_epistemic_relation(
                row.try_get("epistemic_relation").map_err(db)?,
            )?,
            target_revision: parse_reference(
                &row.try_get::<String, _>("basis_kind").map_err(db)?,
                &row.try_get::<String, _>("basis_ref").map_err(db)?,
            )?,
            basis_role: parse_enum(
                ErrorOwner::Concept,
                row.try_get("basis_role").map_err(db)?,
                "schema dependency basis role",
            )?,
        })
    };
    Ok(SchemaEvidenceLink {
        link_id: SchemaEvidenceLinkId(row.try_get("link_id").map_err(db)?),
        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
        schema_revision_id: revision,
        role: parse_enum(
            ErrorOwner::Concept,
            row.try_get("role").map_err(db)?,
            "schema evidence role",
        )?,
        basis,
        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        revoked_at: row.try_get("revoked_at").map_err(db)?,
    })
}
