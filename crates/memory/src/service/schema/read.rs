// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::service::cognition::CognitionSelection;
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
        self.read_schema(
            subject,
            CognitiveRef::CognitiveSchemaRevision(revision),
            None,
        )
        .await
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
        self.read_schema(subject, reference, Some(schema_id)).await
    }

    async fn read_schema(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        expected_schema: Option<CognitiveSchemaId>,
    ) -> Result<SchemaView> {
        let context = self.current_read_context(subject).await?;
        match self
            .read_cognitions(&context, std::slice::from_ref(&reference))
            .await?
            .remove(&reference)
        {
            Some(CognitionView::CognitiveSchema(view))
                if expected_schema.is_none_or(|id| view.header.object_id() == id.0) =>
            {
                Ok(view)
            }
            _ => Err(Error::new(
                ErrorOwner::Concept,
                ErrorCode::NotFound,
                "CognitiveSchema not found",
            )),
        }
    }

    pub(in crate::service) async fn schema_views_for_selections_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        selections: &[CognitionSelection],
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, SchemaView>> {
        let revision_ids = selections
            .iter()
            .map(|selection| selection.revision.revision_id())
            .collect::<Vec<_>>();
        if revision_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query("SELECT schema_revision_id,revision_intent,title,structural_claim,applicability_description,aboutness,tags,boundary_definition,formation_kind,valid_time_kind,valid_time_start,valid_time_end FROM cognitive_schema_revisions WHERE schema_revision_id=ANY($1)")
            .bind(&revision_ids)
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?;
        let historical_link_ids = historical
            .map(|snapshot| snapshot.schema_evidence_links.clone())
            .unwrap_or_default();
        let link_rows = sqlx::query("SELECT link_id,subject_id,schema_revision_id,role,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,producer_signature_id,created_at,revoked_at,epistemic_relation FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=ANY($2) AND (($3 AND link_id=ANY($4::uuid[])) OR (NOT $3 AND revoked_at IS NULL)) ORDER BY schema_revision_id,link_id")
            .bind(subject.0)
            .bind(&revision_ids)
            .bind(historical.is_some())
            .bind(&historical_link_ids)
            .fetch_all(&mut **tx)
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
        let selected = selections
            .iter()
            .map(|selection| (selection.revision.revision_id(), selection))
            .collect::<HashMap<_, _>>();
        let mut views = HashMap::with_capacity(rows.len());
        for row in rows {
            let revision_id: Uuid = row.try_get("schema_revision_id").map_err(db)?;
            let Some(selection) = selected.get(&revision_id) else {
                continue;
            };
            let revision_intent = row
                .try_get::<Option<String>, _>("revision_intent")
                .map_err(db)?
                .map(|value| parse_enum(ErrorOwner::Concept, value, "schema revision intent"))
                .transpose()?;
            let revision = CognitiveSchemaRevision {
                header: selection.revision.clone(),
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
            };
            let reference = selection.revision.revision_ref.clone();
            views.insert(
                reference,
                SchemaView {
                    header: selection.header.clone(),
                    revision,
                    evidence_links: links.remove(&revision_id).unwrap_or_default(),
                },
            );
        }
        Ok(views)
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
