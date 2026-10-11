// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

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
        let schema_id: Uuid = sqlx::query_scalar("SELECT r.schema_id FROM cognitive_schema_revisions r JOIN cognitive_schemas s ON s.schema_id=r.schema_id WHERE s.subject_id=$1 AND r.schema_revision_id=$2")
            .bind(subject.0).bind(revision.0).fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::NotFound("CognitiveSchema revision not found".into()))?;
        self.schema_at(subject, CognitiveSchemaId(schema_id), Some(revision))
            .await
    }
    pub(super) async fn schema_at(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
        revision: Option<CognitiveSchemaRevisionId>,
    ) -> Result<SchemaView> {
        let row = sqlx::query("SELECT s.schema_id,s.subject_id,s.current_revision_id,s.object_epoch,s.acceptance_state,s.integrity_state,s.suppression_state,s.purge_state,s.created_at,r.schema_revision_id,r.schema_id AS revision_schema_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.structural_claim,r.applicability_description,r.aboutness,r.tags,r.boundary_definition,r.formation_kind,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.producer_signature_id FROM cognitive_schemas s JOIN cognitive_schema_revisions r ON r.schema_id=s.schema_id AND r.schema_revision_id=COALESCE($3,s.current_revision_id) WHERE s.subject_id=$1 AND s.schema_id=$2")
            .bind(subject.0).bind(schema_id.0).bind(revision.map(|id| id.0)).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(||Error::NotFound("CognitiveSchema not found".into()))?;
        let revision_id = CognitiveSchemaRevisionId(row.try_get("schema_revision_id").map_err(db)?);
        let revision = CognitiveSchemaRevision {
            schema_revision_id: revision_id,
            schema_id,
            revision_no: row.try_get("revision_no").map_err(db)?,
            parent_revision_id: row
                .try_get::<Option<Uuid>, _>("parent_revision_id")
                .map_err(db)?
                .map(CognitiveSchemaRevisionId),
            revision_intent: row
                .try_get::<Option<String>, _>("revision_intent")
                .map_err(db)?
                .map(|v| parse_enum(v, "schema revision intent"))
                .transpose()?,
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
                    row.try_get("valid_time_kind").map_err(db)?,
                    row.try_get("valid_time_start").map_err(db)?,
                    row.try_get("valid_time_end").map_err(db)?,
                )?,
            },
            boundary_definition: row.try_get("boundary_definition").map_err(db)?,
            formation_kind: parse_enum(
                row.try_get("formation_kind").map_err(db)?,
                "schema formation kind",
            )?,
            formed_at: row.try_get("formed_at").map_err(db)?,
            recorded_at: row.try_get("recorded_at").map_err(db)?,
            producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
        };
        let links = self.schema_links(subject, revision_id).await?;
        Ok(SchemaView {
            schema: CognitiveSchema {
                schema_id,
                subject_id: subject,
                current_revision_id: revision_id,
                object_epoch: row.try_get("object_epoch").map_err(db)?,
                acceptance_state: parse_enum(
                    row.try_get("acceptance_state").map_err(db)?,
                    "schema acceptance state",
                )?,
                integrity_state: parse_enum(
                    row.try_get("integrity_state").map_err(db)?,
                    "schema integrity state",
                )?,
                suppression_state: parse_enum(
                    row.try_get("suppression_state").map_err(db)?,
                    "schema suppression state",
                )?,
                purge_state: parse_enum(
                    row.try_get("purge_state").map_err(db)?,
                    "schema purge state",
                )?,
                created_at: row.try_get("created_at").map_err(db)?,
            },
            revision,
            evidence_links: links,
        })
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
            .map(|row| {
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
                            (None, Some(id), None) => EvidenceLocator::DerivedRepresentation(
                                nous_core::DerivedRepresentationId(id),
                            ),
                            (None, None, Some(id)) => {
                                EvidenceLocator::DerivedRegion(nous_core::DerivedRegionId(id))
                            }
                            (None, None, None) => EvidenceLocator::WholeOccurrence,
                            _ => {
                                return Err(Error::Infrastructure(
                                    "schema evidence locator is invalid".into(),
                                ));
                            }
                        },
                        basis_role: parse_enum(
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
                            row.try_get("basis_role").map_err(db)?,
                            "schema dependency basis role",
                        )?,
                    })
                };
                Ok(SchemaEvidenceLink {
                    link_id: SchemaEvidenceLinkId(row.try_get("link_id").map_err(db)?),
                    subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
                    schema_revision_id: revision,
                    role: parse_enum(row.try_get("role").map_err(db)?, "schema evidence role")?,
                    basis,
                    producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
                    created_at: row.try_get("created_at").map_err(db)?,
                    revoked_at: row.try_get("revoked_at").map_err(db)?,
                })
            })
            .collect()
    }
}
