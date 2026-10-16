// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub async fn schema_page(
        &self,
        subject: SubjectId,
        schema: Option<CognitiveSchemaId>,
        status: &str,
        limit: i64,
        after: Option<Uuid>,
    ) -> Result<(Vec<SchemaView>, bool)> {
        if !(1..=200).contains(&limit) || !matches!(status, "" | "accepted" | "withdrawn") {
            return Err(Error::Invalid("invalid Schema page envelope".into()));
        }
        self.store.require_subject(subject).await?;
        let mut tx = self.store.read_view().await?;
        let mut ids: Vec<Uuid> = if let Some(schema) = schema {
            let head: Option<Uuid> = sqlx::query_scalar("SELECT current_revision_id FROM cognitive_schemas WHERE subject_id=$1 AND schema_id=$2")
                .bind(subject.0).bind(schema.0).fetch_optional(&mut *tx).await.map_err(db)?;
            if head.is_none() {
                return Err(Error::NotFound("Schema not found".into()));
            }
            let after_no = if let Some(after) = after {
                Some(sqlx::query_scalar::<_,i32>("SELECT revision_no FROM cognitive_schema_revisions WHERE schema_id=$1 AND schema_revision_id=$2")
                .bind(schema.0).bind(after).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(|| Error::Invalid("invalid Schema history cursor".into()))?)
            } else {
                None
            };
            sqlx::query_scalar("SELECT schema_revision_id FROM cognitive_schema_revisions WHERE schema_id=$1 AND ($2::int IS NULL OR revision_no>$2) ORDER BY revision_no LIMIT $3")
                .bind(schema.0).bind(after_no).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?
        } else {
            sqlx::query_scalar("SELECT current_revision_id FROM cognitive_schemas WHERE subject_id=$1 AND ($2='' OR acceptance_state=$2) AND ($3::uuid IS NULL OR schema_id>$3) ORDER BY schema_id LIMIT $4")
                .bind(subject.0).bind(status).bind(after).bind(limit+1).fetch_all(&mut *tx).await.map_err(db)?
        };
        let more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        let mut views = self.schema_views_in(&mut tx, subject, &ids, &[]).await?;
        let items = ids
            .into_iter()
            .map(|id| {
                views
                    .remove(&id)
                    .ok_or_else(|| Error::NotFound("Schema page not found".into()))
            })
            .collect::<Result<_>>()?;
        tx.commit().await.map_err(db)?;
        Ok((items, more))
    }
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
        self.schema_revision_views(subject, &[revision])
            .await?
            .pop()
            .ok_or_else(|| Error::NotFound("CognitiveSchema revision not found".into()))
    }
    pub(in crate::service) async fn schema_at(
        &self,
        subject: SubjectId,
        schema_id: CognitiveSchemaId,
        revision: Option<CognitiveSchemaRevisionId>,
    ) -> Result<SchemaView> {
        let mut tx = self.store.read_view().await?;
        let revisions = revision.map(|id| vec![id.0]).unwrap_or_default();
        let objects = if revision.is_none() {
            vec![schema_id.0]
        } else {
            vec![]
        };
        let views = self
            .schema_views_in(&mut tx, subject, &revisions, &objects)
            .await?;
        tx.commit().await.map_err(db)?;
        views
            .into_values()
            .find(|view| view.schema.schema_id == schema_id)
            .ok_or_else(|| Error::NotFound("CognitiveSchema not found".into()))
    }

    pub(in crate::service) async fn schema_revision_views(
        &self,
        subject: SubjectId,
        revisions: &[CognitiveSchemaRevisionId],
    ) -> Result<Vec<SchemaView>> {
        let ids = revisions.iter().map(|id| id.0).collect::<Vec<_>>();
        let mut tx = self.store.read_view().await?;
        let mut views = self.schema_views_in(&mut tx, subject, &ids, &[]).await?;
        let result = revisions
            .iter()
            .map(|id| {
                views
                    .remove(&id.0)
                    .ok_or_else(|| Error::NotFound("CognitiveSchema revision not found".into()))
            })
            .collect();
        tx.commit().await.map_err(db)?;
        result
    }

    pub(in crate::service) async fn schema_views_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        ids: &[Uuid],
        objects: &[Uuid],
    ) -> Result<std::collections::HashMap<Uuid, SchemaView>> {
        let rows = sqlx::query("SELECT s.schema_id,s.subject_id,s.current_revision_id,s.object_epoch,s.acceptance_state,s.integrity_state,s.suppression_state,s.created_at,r.schema_revision_id,r.schema_id AS revision_schema_id,r.revision_no,r.parent_revision_id,r.revision_intent,r.title,r.structural_claim,r.applicability_description,r.aboutness,r.tags,r.boundary_definition,r.formation_kind,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.formed_time_domain_ref,r.recorded_at,r.producer_signature_id FROM cognitive_schemas s JOIN cognitive_schema_revisions r ON r.schema_id=s.schema_id WHERE s.subject_id=$1 AND (r.schema_revision_id=ANY($2::uuid[]) OR (s.schema_id=ANY($3) AND r.schema_revision_id=s.current_revision_id))")
            .bind(subject.0).bind(ids).bind(objects).fetch_all(&mut **tx).await.map_err(db)?;
        let selected = rows
            .iter()
            .map(|row| row.try_get::<Uuid, _>("schema_revision_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let link_rows = sqlx::query("SELECT * FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND schema_revision_id=ANY($2::uuid[]) AND revoked_at IS NULL ORDER BY schema_revision_id,link_id")
            .bind(subject.0).bind(&selected).fetch_all(&mut **tx).await.map_err(db)?;
        let mut links = std::collections::HashMap::<Uuid, Vec<SchemaEvidenceLink>>::new();
        for row in link_rows {
            let link = decode_schema_evidence_link(row)?;
            links
                .entry(link.schema_revision_id.0)
                .or_default()
                .push(link);
        }
        let mut views = std::collections::HashMap::new();
        for row in rows {
            let revision: Uuid = row.try_get("schema_revision_id").map_err(db)?;
            views.insert(
                revision,
                decode_schema_view(&row, subject, links.remove(&revision).unwrap_or_default())?,
            );
        }
        Ok(views)
    }
}

fn decode_schema_view(
    row: &sqlx::postgres::PgRow,
    subject: SubjectId,
    links: Vec<SchemaEvidenceLink>,
) -> Result<SchemaView> {
    let schema_id = CognitiveSchemaId(row.try_get("schema_id").map_err(db)?);
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
        formed_time_domain_ref: row.try_get("formed_time_domain_ref").map_err(db)?,
        recorded_at: row.try_get("recorded_at").map_err(db)?,
        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
    };
    Ok(SchemaView {
        schema: CognitiveSchema {
            schema_id,
            subject_id: subject,
            current_revision_id: CognitiveSchemaRevisionId(
                row.try_get("current_revision_id").map_err(db)?,
            ),
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
            created_at: row.try_get("created_at").map_err(db)?,
        },
        revision,
        evidence_links: links,
    })
}

fn decode_schema_evidence_link(row: sqlx::postgres::PgRow) -> Result<SchemaEvidenceLink> {
    let content = decode_schema_link_input(&row)?;
    Ok(SchemaEvidenceLink {
        link_id: SchemaEvidenceLinkId(row.try_get("link_id").map_err(db)?),
        subject_id: SubjectId(row.try_get("subject_id").map_err(db)?),
        schema_revision_id: CognitiveSchemaRevisionId(
            row.try_get("schema_revision_id").map_err(db)?,
        ),
        role: content.role,
        basis: content.basis,
        producer_signature_id: row.try_get("producer_signature_id").map_err(db)?,
        created_at: row.try_get("created_at").map_err(db)?,
        revoked_at: row.try_get("revoked_at").map_err(db)?,
    })
}
