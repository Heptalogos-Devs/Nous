//! Coherent read snapshots used to build disposable serving artifacts.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{AuthorityStore, database_error as db};
use nous_core::*;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextProjectionSource {
    pub reference: CognitiveRef,
    pub text: Option<String>,
    pub member_fragments: Vec<TextProjectionFragment>,
    pub title: Option<String>,
    pub source_class: Option<String>,
    pub source_region: Option<SourceRegionId>,
    pub entity_refs: Vec<String>,
    pub tag_ids: Vec<String>,
    pub schema_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextProjectionFragment {
    pub reference: CognitiveRef,
}

pub struct TextProjectionInput {
    pub watermark: i64,
    pub sources: Vec<TextProjectionSource>,
}

pub(crate) async fn watermark(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    family: &str,
    space: &str,
) -> Result<i64> {
    sqlx::query_scalar("SELECT COALESCE(max(desired_authority_seq),0) FROM projection_watermarks WHERE subject_id=$1 AND family=$2 AND (space_signature=$3 OR space_signature='*')")
        .bind(subject.0).bind(family).bind(space).fetch_one(&mut **tx).await.map_err(db)
}

impl AuthorityStore {
    pub async fn projection_watermark(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
    ) -> Result<i64> {
        let mut tx = self.begin().await?;
        watermark(&mut tx, subject, family, space).await
    }

    pub async fn text_projection_input(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
        memory_enabled: bool,
        episode_budget: crate::EpisodeTextBudget,
    ) -> Result<TextProjectionInput> {
        let mut tx = self.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        let watermark = watermark(&mut tx, subject, family, space).await?;
        let mut sources = material_sources(&mut tx, subject, None).await?;
        if memory_enabled {
            sources.extend(memory_sources(&mut tx, subject, None).await?);
            sources.extend(longitudinal_sources(&mut tx, subject, episode_budget, None).await?);
        }
        sources.sort_by_key(|source| source.reference.to_string());
        tx.commit().await.map_err(db)?;
        Ok(TextProjectionInput { watermark, sources })
    }
}

pub(crate) async fn memory_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    let rows = sqlx::query("SELECT o.memory_id,r.memory_revision_id,r.representation_text,r.title FROM memory_objects o JOIN memory_revisions r ON r.memory_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND ($2::uuid[] IS NULL OR r.memory_revision_id=ANY($2)) ORDER BY o.memory_id")
        .bind(subject.0).bind(selected_ids(references, "memory_revision")).fetch_all(&mut **tx).await.map_err(db)?;
    let mut sources = Vec::new();
    for row in rows {
        let revision: Uuid = row.try_get("memory_revision_id").map_err(db)?;
        let entities = sqlx::query_scalar("SELECT DISTINCT entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=$1 ORDER BY entity_ref")
            .bind(revision).fetch_all(&mut **tx).await.map_err(db)?;
        let tags = sqlx::query_scalar("SELECT DISTINCT canonical_tag($2,tag_id)::text FROM memory_revision_tags WHERE memory_revision_id=$1 AND canonical_tag($2,tag_id) IS NOT NULL ORDER BY 1")
            .bind(revision).bind(subject.0).fetch_all(&mut **tx).await.map_err(db)?;
        let schema_ids = sqlx::query_scalar("SELECT DISTINCT s.schema_id::text FROM cognitive_schemas s JOIN cognitive_schema_revisions sr ON sr.schema_revision_id=s.current_revision_id JOIN cognitive_schema_evidence_links l ON l.schema_revision_id=sr.schema_revision_id WHERE s.subject_id=$1 AND s.acceptance_state='accepted' AND s.integrity_state='valid' AND s.suppression_state='normal' AND s.purge_state='normal' AND l.revoked_at IS NULL AND l.role='support' AND l.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND l.basis_kind='memory_revision' AND l.basis_ref=$2")
            .bind(subject.0).bind(revision.to_string()).fetch_all(&mut **tx).await.map_err(db)?;
        sources.push(TextProjectionSource {
            reference: CognitiveRef::MemoryRevision(MemoryRevisionId(revision)),
            text: Some(row.try_get("representation_text").map_err(db)?),
            member_fragments: Vec::new(),
            title: row.try_get("title").map_err(db)?,
            source_class: None,
            source_region: None,
            entity_refs: entities,
            tag_ids: tags,
            schema_ids,
        });
    }
    for (table, sql) in [
        (
            "tags",
            "SELECT o.tag_id AS id,NULL::uuid AS revision_id,r.label,r.description,r.kind_hint FROM tags o JOIN tag_revisions r ON r.tag_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.status='active' AND ($2::uuid[] IS NULL OR o.tag_id=ANY($2))",
        ),
        (
            "cognitive_schemas",
            "SELECT o.schema_id AS id,r.schema_revision_id AS revision_id,r.structural_claim AS label,NULL::text AS description,NULL::text AS kind_hint FROM cognitive_schemas o JOIN cognitive_schema_revisions r ON r.schema_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND ($2::uuid[] IS NULL OR r.schema_revision_id=ANY($2))",
        ),
    ] {
        for row in sqlx::query(sql)
            .bind(subject.0)
            .bind(selected_ids(
                references,
                if table == "tags" {
                    "tag"
                } else {
                    "cognitive_schema_revision"
                },
            ))
            .fetch_all(&mut **tx)
            .await
            .map_err(db)?
        {
            let id: Uuid = row.try_get("id").map_err(db)?;
            let revision: Option<Uuid> = row.try_get("revision_id").map_err(db)?;
            let reference = if table == "tags" {
                CognitiveRef::Tag(TagId(id))
            } else {
                CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(
                    revision
                        .ok_or_else(|| Error::Infrastructure("schema revision missing".into()))?,
                ))
            };
            sources.push(TextProjectionSource {
                reference,
                text: Some(if table == "tags" {
                    tag_semantic_representation(
                        &row.try_get::<String, _>("label").map_err(db)?,
                        row.try_get::<Option<String>, _>("description")
                            .map_err(db)?
                            .as_deref(),
                        row.try_get::<Option<String>, _>("kind_hint")
                            .map_err(db)?
                            .as_deref(),
                    )?
                    .text
                } else {
                    row.try_get("label").map_err(db)?
                }),
                member_fragments: Vec::new(),
                title: None,
                source_class: None,
                source_region: None,
                entity_refs: Vec::new(),
                tag_ids: Vec::new(),
                schema_ids: Vec::new(),
            });
        }
    }
    Ok(sources)
}

pub(crate) async fn material_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    let rows = sqlx::query("SELECT o.occurrence_id,o.source_class,o.artifact_id FROM observation_occurrences o WHERE o.subject_id=$1 AND ($2::uuid[] IS NULL OR o.occurrence_id=ANY($2)) ORDER BY o.occurrence_id")
        .bind(subject.0).bind(selected_ids(references, "occurrence")).fetch_all(&mut **tx).await.map_err(db)?;
    let mut sources = Vec::new();
    for row in rows {
        let occurrence: Uuid = row.try_get("occurrence_id").map_err(db)?;
        let entities = sqlx::query_scalar("SELECT DISTINCT b.entity_ref FROM entity_mentions m JOIN entity_binding_revisions b USING(mention_id) WHERE m.occurrence_id=$1 AND b.revision_no=(SELECT max(b2.revision_no) FROM entity_binding_revisions b2 WHERE b2.mention_id=b.mention_id) AND b.binding_state='bound' AND b.entity_ref IS NOT NULL")
            .bind(occurrence).fetch_all(&mut **tx).await.map_err(db)?;
        let region: Option<Uuid> = sqlx::query_scalar("SELECT source_region_id FROM source_regions WHERE artifact_id=$1 AND coordinate_kind='whole_artifact' ORDER BY source_region_id LIMIT 1")
            .bind(row.try_get::<Option<Uuid>,_>("artifact_id").map_err(db)?).fetch_optional(&mut **tx).await.map_err(db)?;
        sources.push(TextProjectionSource {
            reference: CognitiveRef::Occurrence(OccurrenceId(occurrence)),
            text: None,
            member_fragments: Vec::new(),
            title: None,
            source_class: Some(row.try_get("source_class").map_err(db)?),
            source_region: region.map(SourceRegionId),
            entity_refs: entities,
            tag_ids: Vec::new(),
            schema_ids: Vec::new(),
        });
    }
    let rows = sqlx::query("SELECT r.source_region_id,r.created_at,o.source_class FROM source_regions r JOIN artifacts a ON a.artifact_id=r.artifact_id LEFT JOIN LATERAL (SELECT source_class FROM observation_occurrences WHERE subject_id=r.subject_id AND artifact_id=r.artifact_id ORDER BY observed_at DESC LIMIT 1) o ON true WHERE r.subject_id=$1 AND ($2::uuid[] IS NULL OR r.source_region_id=ANY($2)) ORDER BY r.source_region_id")
        .bind(subject.0).bind(selected_ids(references, "source_region"))
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    for row in rows {
        sources.push(TextProjectionSource {
            reference: CognitiveRef::SourceRegion(SourceRegionId(
                row.try_get("source_region_id").map_err(db)?,
            )),
            text: None,
            member_fragments: Vec::new(),
            title: None,
            source_class: row.try_get("source_class").map_err(db)?,
            source_region: Some(SourceRegionId(row.try_get("source_region_id").map_err(db)?)),
            entity_refs: Vec::new(),
            tag_ids: Vec::new(),
            schema_ids: Vec::new(),
        });
    }
    let rows = sqlx::query("SELECT d.derived_representation_id,d.representation_kind FROM derived_representations d WHERE d.subject_id=$1 AND ($2::uuid[] IS NULL OR d.derived_representation_id=ANY($2)) ORDER BY d.derived_representation_id")
        .bind(subject.0).bind(selected_ids(references, "derived_representation")).fetch_all(&mut **tx).await.map_err(db)?;
    for row in rows {
        sources.push(TextProjectionSource {
            reference: CognitiveRef::DerivedRepresentation(DerivedRepresentationId(
                row.try_get("derived_representation_id").map_err(db)?,
            )),
            text: None,
            member_fragments: Vec::new(),
            title: Some(row.try_get("representation_kind").map_err(db)?),
            source_class: Some("derived".into()),
            source_region: None,
            entity_refs: Vec::new(),
            tag_ids: Vec::new(),
            schema_ids: Vec::new(),
        });
    }
    let rows = sqlx::query("SELECT r.derived_region_id,r.created_at,d.representation_kind FROM derived_regions r JOIN derived_representations d ON d.derived_representation_id=r.derived_representation_id WHERE r.subject_id=$1 AND ($2::uuid[] IS NULL OR r.derived_region_id=ANY($2)) ORDER BY r.derived_region_id")
        .bind(subject.0).bind(selected_ids(references, "derived_region"))
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    for row in rows {
        sources.push(TextProjectionSource {
            reference: CognitiveRef::DerivedRegion(DerivedRegionId(
                row.try_get("derived_region_id").map_err(db)?,
            )),
            text: None,
            member_fragments: Vec::new(),
            title: Some(row.try_get("representation_kind").map_err(db)?),
            source_class: Some("derived".into()),
            source_region: None,
            entity_refs: Vec::new(),
            tag_ids: Vec::new(),
            schema_ids: Vec::new(),
        });
    }
    Ok(sources)
}

pub(crate) async fn longitudinal_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    episode_budget: crate::EpisodeTextBudget,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    let episodes=sqlx::query("SELECT r.episode_revision_id,r.title,r.boundary_explanation,r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM episode_objects o JOIN episode_revisions r ON r.episode_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND ($2::uuid[] IS NULL OR r.episode_revision_id=ANY($2)) ORDER BY o.episode_id")
        .bind(subject.0).bind(selected_ids(references, "episode_revision")).fetch_all(&mut **tx).await.map_err(db)?;
    let ids: Vec<Uuid> = episodes
        .iter()
        .map(|row| row.get("episode_revision_id"))
        .collect();
    let mut member_fragments =
        crate::episode_text::episode_member_text_input_in(tx, subject, &ids, episode_budget)
            .await?;
    let mut sources = Vec::with_capacity(episodes.len());
    for row in episodes {
        let revision: Uuid = row.get("episode_revision_id");
        let title: Option<String> = row.get("title");
        sources.push(TextProjectionSource {
            reference: CognitiveRef::EpisodeRevision(EpisodeRevisionId(revision)),
            text: Some(episode_representation(&row)),
            member_fragments: member_fragments.remove(&revision).unwrap_or_default(),
            title,
            source_class: None,
            source_region: None,
            entity_refs: vec![],
            tag_ids: vec![],
            schema_ids: vec![],
        });
    }
    let journals=sqlx::query("SELECT r.journal_revision_id,r.title,r.narrative,COALESCE((SELECT string_agg(text,E'\n' ORDER BY ordinal) FROM journal_revision_points WHERE journal_revision_id=r.journal_revision_id),'') AS points FROM journal_objects o JOIN journal_revisions r ON r.journal_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal' AND ($2::uuid[] IS NULL OR r.journal_revision_id=ANY($2)) ORDER BY o.journal_id")
        .bind(subject.0).bind(selected_ids(references, "journal_revision")).fetch_all(&mut **tx).await.map_err(db)?;
    for row in journals {
        let revision: Uuid = row.get("journal_revision_id");
        let title: Option<String> = row.get("title");
        let mut text = format!(
            "{}\n{}\n{}",
            title.as_deref().unwrap_or_default(),
            row.get::<String, _>("narrative"),
            row.get::<String, _>("points")
        );
        truncate_text(&mut text, 65536);
        sources.push(TextProjectionSource {
            reference: CognitiveRef::JournalRevision(JournalRevisionId(revision)),
            text: Some(text),
            member_fragments: vec![],
            title,
            source_class: None,
            source_region: None,
            entity_refs: vec![],
            tag_ids: vec![],
            schema_ids: vec![],
        });
    }
    Ok(sources)
}
pub(crate) fn truncate_text(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn selected_ids(references: Option<&[CognitiveRef]>, kind: &str) -> Option<Vec<Uuid>> {
    references.map(|references| {
        references
            .iter()
            .filter_map(|reference| {
                let (actual, value) = reference_parts(reference);
                (actual == kind).then(|| value.parse().expect("projection identity is UUID"))
            })
            .collect()
    })
}

pub(crate) fn episode_representation(row: &sqlx::postgres::PgRow) -> String {
    let title: Option<String> = row.get("title");
    let start: Option<chrono::DateTime<chrono::Utc>> = row.get("experience_time_start");
    let end: Option<chrono::DateTime<chrono::Utc>> = row.get("experience_time_end");
    format!(
        "{}\n{}\nExperience {} {:?} {:?}",
        title.as_deref().unwrap_or_default(),
        row.get::<String, _>("boundary_explanation"),
        row.get::<String, _>("experience_time_kind"),
        start,
        end
    )
}
