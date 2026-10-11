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
        let sources = current_sources_in(
            &mut tx,
            subject,
            if memory_enabled {
                CurrentCognitionSet::All
            } else {
                CurrentCognitionSet::None
            },
            true,
            memory_enabled.then_some(episode_budget),
            None,
        )
        .await?;
        tx.commit().await.map_err(db)?;
        Ok(TextProjectionInput { watermark, sources })
    }
}

/// Select the current corpus first, then materialize each selected reference through the same
/// immutable-row catalog used by historical projections. Current eligibility remains here.
#[derive(Clone, Copy)]
pub(crate) enum CurrentCognitionSet {
    None,
    Memory,
    Longitudinal,
    All,
}

pub(crate) async fn current_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    cognition: CurrentCognitionSet,
    include_material: bool,
    episode_budget: Option<crate::EpisodeTextBudget>,
    requested: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    let references =
        current_references_in(tx, subject, cognition, include_material, requested).await?;
    let mut sources = catalog_sources_in(tx, subject, &references).await?;
    current_tag_sources_in(tx, subject, &references, &mut sources).await?;
    enrich_current_sources_in(tx, subject, &mut sources).await?;
    if let Some(episode_budget) = episode_budget {
        current_episode_fragments_in(tx, subject, episode_budget, &mut sources).await?;
    }
    sources.sort_by_key(|source| source.reference.to_string());
    Ok(sources)
}

async fn current_references_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    cognition: CurrentCognitionSet,
    include_material: bool,
    requested: Option<&[CognitiveRef]>,
) -> Result<Vec<CognitiveRef>> {
    let (requested_kinds, requested_values): (Option<Vec<_>>, Option<Vec<_>>) =
        requested.map_or((None, None), |references| {
            let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
            (Some(kinds), Some(values))
        });
    let (memory_enabled, cognition_kind) = match cognition {
        CurrentCognitionSet::None => (false, 0),
        CurrentCognitionSet::Memory => (true, 1),
        CurrentCognitionSet::Longitudinal => (true, 2),
        CurrentCognitionSet::All => (true, 3),
    };
    let rows = sqlx::query(r#"
WITH catalog AS (
 SELECT 'occurrence'::text kind,occurrence_id::text value FROM observation_occurrences WHERE $2 AND subject_id=$1
 UNION ALL SELECT 'source_region',source_region_id::text FROM source_regions WHERE $2 AND subject_id=$1
 UNION ALL SELECT 'derived_representation',derived_representation_id::text FROM derived_representations WHERE $2 AND subject_id=$1
 UNION ALL SELECT 'derived_region',derived_region_id::text FROM derived_regions WHERE $2 AND subject_id=$1
 UNION ALL SELECT CASE object_kind
                   WHEN 'memory' THEN 'memory_revision'
                   WHEN 'cognitive_schema' THEN 'cognitive_schema_revision'
                   WHEN 'episode' THEN 'episode_revision'
                   WHEN 'journal' THEN 'journal_revision'
                 END,current_revision_id::text
            FROM cognition_objects
            WHERE $3 AND subject_id=$1 AND acceptance_state='accepted' AND integrity_state='valid'
              AND suppression_state='normal' AND purge_state='normal' AND current_revision_id IS NOT NULL
              AND ((($4 IN (1,3)) AND object_kind IN ('memory','cognitive_schema'))
                OR (($4 IN (2,3)) AND object_kind IN ('episode','journal')))
 UNION ALL SELECT 'tag',tag_id::text FROM tags WHERE $3 AND $4 IN (1,3) AND subject_id=$1 AND status='active'
)
SELECT DISTINCT kind,value FROM catalog
WHERE ($5::text[] IS NULL OR (kind,value) IN (SELECT * FROM unnest($5::text[],$6::text[])))
ORDER BY kind,value
"#)
        .bind(subject.0)
        .bind(include_material)
        .bind(memory_enabled)
        .bind(cognition_kind)
        .bind(requested_kinds)
        .bind(requested_values)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    rows.into_iter()
        .map(|row| {
            parse_reference(
                &row.try_get::<String, _>("kind").map_err(db)?,
                &row.try_get::<String, _>("value").map_err(db)?,
            )
        })
        .collect()
}

/// The only revision/material body reader. Both current and as-of selectors pass exact refs here;
/// current purge is a hard fence for frozen cognition revisions.
pub(crate) async fn catalog_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: &[CognitiveRef],
) -> Result<Vec<TextProjectionSource>> {
    let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
    let rows = sqlx::query(r#"
WITH selected AS (SELECT * FROM unnest($2::text[],$3::text[]) r(kind,value)), cognition AS (
 SELECT s.kind,s.value,h.revision_id,h.object_kind
 FROM selected s
 JOIN cognition_revision_headers h ON h.revision_id::text=s.value AND (
     (s.kind='memory_revision' AND h.object_kind='memory')
  OR (s.kind='cognitive_schema_revision' AND h.object_kind='cognitive_schema')
  OR (s.kind='episode_revision' AND h.object_kind='episode')
  OR (s.kind='journal_revision' AND h.object_kind='journal'))
 JOIN cognition_objects o ON o.subject_id=h.subject_id AND o.object_kind=h.object_kind AND o.object_id=h.object_id
 WHERE h.subject_id=$1 AND o.purge_state='normal'
), catalog AS (
 SELECT c.kind,c.value,r.title,r.representation_text text,NULL::text source_class FROM cognition c JOIN memory_revisions r ON c.object_kind='memory' AND r.memory_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,r.title,r.structural_claim,NULL FROM cognition c JOIN cognitive_schema_revisions r ON c.object_kind='cognitive_schema' AND r.schema_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,r.title,NULL,NULL FROM cognition c JOIN episode_revisions r ON c.object_kind='episode' AND r.episode_revision_id=c.revision_id
 UNION ALL SELECT c.kind,c.value,r.title,COALESCE(r.title,'')||E'\n'||r.narrative||E'\n'||COALESCE((SELECT string_agg(text,E'\n' ORDER BY ordinal) FROM journal_revision_points WHERE journal_revision_id=r.journal_revision_id),''),NULL FROM cognition c JOIN journal_revisions r ON c.object_kind='journal' AND r.journal_revision_id=c.revision_id
 UNION ALL SELECT s.kind,s.value,NULL,NULL,o.source_class FROM selected s JOIN observation_occurrences o ON s.kind='occurrence' AND o.occurrence_id=CASE WHEN s.kind='occurrence' THEN s.value::uuid END LEFT JOIN artifacts a USING(artifact_id) WHERE o.subject_id=$1
 UNION ALL SELECT s.kind,s.value,NULL,NULL,NULL FROM selected s JOIN source_regions r ON s.kind='source_region' AND r.source_region_id=CASE WHEN s.kind='source_region' THEN s.value::uuid END JOIN artifacts a USING(artifact_id) WHERE r.subject_id=$1
 UNION ALL SELECT s.kind,s.value,d.representation_kind,NULL,'derived' FROM selected s JOIN derived_representations d ON s.kind='derived_representation' AND d.derived_representation_id=CASE WHEN s.kind='derived_representation' THEN s.value::uuid END WHERE d.subject_id=$1
 UNION ALL SELECT s.kind,s.value,d.representation_kind,NULL,'derived' FROM selected s JOIN derived_regions r ON s.kind='derived_region' AND r.derived_region_id=CASE WHEN s.kind='derived_region' THEN s.value::uuid END JOIN derived_representations d USING(derived_representation_id) WHERE r.subject_id=$1
) SELECT * FROM catalog ORDER BY kind,value
"#)
        .bind(subject.0)
        .bind(kinds)
        .bind(values)
        .fetch_all(&mut **tx)
        .await
        .map_err(db)?;
    let mut sources = Vec::new();
    for row in rows {
        let reference = parse_reference(
            &row.try_get::<String, _>("kind").map_err(db)?,
            &row.try_get::<String, _>("value").map_err(db)?,
        )?;
        sources.push(TextProjectionSource {
            source_region: match &reference {
                CognitiveRef::SourceRegion(id) => Some(*id),
                _ => None,
            },
            reference,
            text: row.try_get("text").map_err(db)?,
            title: row.try_get("title").map_err(db)?,
            source_class: row.try_get("source_class").map_err(db)?,
            member_fragments: vec![],
            entity_refs: vec![],
            tag_ids: vec![],
            schema_ids: vec![],
        });
    }
    let episode_ids: Vec<_> = sources
        .iter()
        .filter_map(|source| match source.reference {
            CognitiveRef::EpisodeRevision(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let rows = sqlx::query("SELECT r.episode_revision_id,r.title,r.boundary_explanation,r.experience_time_kind,r.experience_time_start,r.experience_time_end FROM episode_revisions r JOIN cognition_revision_headers h ON h.revision_id=r.episode_revision_id AND h.object_kind='episode' JOIN cognition_objects o ON o.subject_id=h.subject_id AND o.object_kind=h.object_kind AND o.object_id=h.object_id WHERE h.subject_id=$1 AND o.purge_state='normal' AND r.episode_revision_id=ANY($2::uuid[])")
        .bind(subject.0).bind(&episode_ids).fetch_all(&mut **tx).await.map_err(db)?;
    for row in rows {
        let reference = CognitiveRef::EpisodeRevision(EpisodeRevisionId(
            row.try_get("episode_revision_id").map_err(db)?,
        ));
        if let Some(source) = sources
            .iter_mut()
            .find(|source| source.reference == reference)
        {
            source.text = Some(episode_representation(&row));
        }
    }
    for source in &mut sources {
        if matches!(source.reference, CognitiveRef::JournalRevision(_))
            && let Some(text) = &mut source.text
        {
            truncate_text(text, 65536);
        }
    }
    Ok(sources)
}

async fn current_tag_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: &[CognitiveRef],
    sources: &mut Vec<TextProjectionSource>,
) -> Result<()> {
    let tag_ids: Vec<_> = references
        .iter()
        .filter_map(|reference| match reference {
            CognitiveRef::Tag(tag) => Some(tag.0),
            _ => None,
        })
        .collect();
    let rows = sqlx::query("SELECT o.tag_id,r.label,r.description,r.kind_hint FROM tags o JOIN tag_revisions r ON r.tag_revision_id=o.current_revision_id WHERE o.subject_id=$1 AND o.status='active' AND o.tag_id=ANY($2::uuid[]) ORDER BY o.tag_id")
        .bind(subject.0).bind(tag_ids).fetch_all(&mut **tx).await.map_err(db)?;
    for row in rows {
        let label: String = row.try_get("label").map_err(db)?;
        let description: Option<String> = row.try_get("description").map_err(db)?;
        let kind_hint: Option<String> = row.try_get("kind_hint").map_err(db)?;
        sources.push(TextProjectionSource {
            reference: CognitiveRef::Tag(TagId(row.try_get("tag_id").map_err(db)?)),
            text: Some(
                tag_semantic_representation(&label, description.as_deref(), kind_hint.as_deref())?
                    .text,
            ),
            member_fragments: vec![],
            title: None,
            source_class: None,
            source_region: None,
            entity_refs: vec![],
            tag_ids: vec![],
            schema_ids: vec![],
        });
    }
    Ok(())
}

async fn enrich_current_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    sources: &mut [TextProjectionSource],
) -> Result<()> {
    for (source, revision) in sources
        .iter_mut()
        .filter_map(|source| match source.reference {
            CognitiveRef::MemoryRevision(id) => Some((source, id.0)),
            _ => None,
        })
    {
        source.entity_refs = sqlx::query_scalar("SELECT DISTINCT entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=$1 ORDER BY entity_ref")
            .bind(revision).fetch_all(&mut **tx).await.map_err(db)?;
        source.tag_ids = sqlx::query_scalar("SELECT DISTINCT canonical_tag($2,tag_id)::text FROM memory_revision_tags WHERE memory_revision_id=$1 AND canonical_tag($2,tag_id) IS NOT NULL ORDER BY 1")
            .bind(revision).bind(subject.0).fetch_all(&mut **tx).await.map_err(db)?;
        source.schema_ids = sqlx::query_scalar("SELECT DISTINCT s.object_id::text FROM cognition_objects s JOIN cognition_revision_headers sr ON sr.subject_id=s.subject_id AND sr.object_kind=s.object_kind AND sr.object_id=s.object_id AND sr.revision_id=s.current_revision_id JOIN cognitive_schema_revisions payload ON payload.schema_revision_id=sr.revision_id JOIN cognitive_schema_evidence_links l ON l.schema_revision_id=payload.schema_revision_id WHERE s.object_kind='cognitive_schema' AND s.subject_id=$1 AND s.acceptance_state='accepted' AND s.integrity_state='valid' AND s.suppression_state='normal' AND s.purge_state='normal' AND l.revoked_at IS NULL AND l.role='support' AND l.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND l.basis_kind='memory_revision' AND l.basis_ref=$2")
            .bind(subject.0).bind(revision.to_string()).fetch_all(&mut **tx).await.map_err(db)?;
    }
    let occurrences: Vec<_> = sources
        .iter()
        .filter_map(|source| match source.reference {
            CognitiveRef::Occurrence(id) => Some(id.0),
            _ => None,
        })
        .collect();
    if !occurrences.is_empty() {
        let rows = sqlx::query("SELECT m.occurrence_id,b.entity_ref FROM entity_mentions m JOIN entity_binding_revisions b USING(mention_id) WHERE m.occurrence_id=ANY($1::uuid[]) AND b.revision_no=(SELECT max(b2.revision_no) FROM entity_binding_revisions b2 WHERE b2.mention_id=b.mention_id) AND b.binding_state='bound' AND b.entity_ref IS NOT NULL ORDER BY m.occurrence_id,b.entity_ref")
            .bind(&occurrences).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let reference =
                CognitiveRef::Occurrence(OccurrenceId(row.try_get("occurrence_id").map_err(db)?));
            if let Some(source) = sources
                .iter_mut()
                .find(|source| source.reference == reference)
            {
                source
                    .entity_refs
                    .push(row.try_get("entity_ref").map_err(db)?);
            }
        }
        let rows = sqlx::query("SELECT o.occurrence_id,r.source_region_id FROM observation_occurrences o LEFT JOIN LATERAL (SELECT source_region_id FROM source_regions WHERE artifact_id=o.artifact_id AND coordinate_kind='whole_artifact' ORDER BY source_region_id LIMIT 1) r ON true WHERE o.subject_id=$1 AND o.occurrence_id=ANY($2::uuid[])")
            .bind(subject.0).bind(&occurrences).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let reference =
                CognitiveRef::Occurrence(OccurrenceId(row.try_get("occurrence_id").map_err(db)?));
            if let Some(source) = sources
                .iter_mut()
                .find(|source| source.reference == reference)
            {
                source.source_region = row
                    .try_get::<Option<Uuid>, _>("source_region_id")
                    .map_err(db)?
                    .map(SourceRegionId);
            }
        }
    }
    let source_regions: Vec<_> = sources
        .iter()
        .filter_map(|source| match source.reference {
            CognitiveRef::SourceRegion(id) => Some(id.0),
            _ => None,
        })
        .collect();
    if !source_regions.is_empty() {
        let rows = sqlx::query("SELECT r.source_region_id,o.source_class FROM source_regions r JOIN artifacts a ON a.artifact_id=r.artifact_id LEFT JOIN LATERAL (SELECT source_class FROM observation_occurrences WHERE subject_id=r.subject_id AND artifact_id=r.artifact_id ORDER BY observed_at DESC LIMIT 1) o ON true WHERE r.subject_id=$1 AND r.source_region_id=ANY($2::uuid[])")
            .bind(subject.0).bind(&source_regions).fetch_all(&mut **tx).await.map_err(db)?;
        for row in rows {
            let reference = CognitiveRef::SourceRegion(SourceRegionId(
                row.try_get("source_region_id").map_err(db)?,
            ));
            if let Some(source) = sources
                .iter_mut()
                .find(|source| source.reference == reference)
            {
                source.source_class = row.try_get("source_class").map_err(db)?;
            }
        }
    }
    Ok(())
}

async fn current_episode_fragments_in(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    episode_budget: crate::EpisodeTextBudget,
    sources: &mut [TextProjectionSource],
) -> Result<()> {
    let episodes: Vec<_> = sources
        .iter()
        .filter_map(|source| match source.reference {
            CognitiveRef::EpisodeRevision(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let mut fragments =
        crate::episode_text::episode_member_text_input_in(tx, subject, &episodes, episode_budget)
            .await?;
    for source in sources {
        if let CognitiveRef::EpisodeRevision(id) = source.reference {
            source.member_fragments = fragments.remove(&id.0).unwrap_or_default();
        }
    }
    Ok(())
}

// Family-scoped wrappers remain for topology callers; all body extraction goes through the catalog.
pub(crate) async fn memory_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    current_sources_in(
        tx,
        subject,
        CurrentCognitionSet::Memory,
        false,
        None,
        references,
    )
    .await
}

pub(crate) async fn material_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    current_sources_in(
        tx,
        subject,
        CurrentCognitionSet::None,
        true,
        None,
        references,
    )
    .await
}

pub(crate) async fn longitudinal_sources(
    tx: &mut Transaction<'_, Postgres>,
    subject: SubjectId,
    episode_budget: crate::EpisodeTextBudget,
    references: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    current_sources_in(
        tx,
        subject,
        CurrentCognitionSet::Longitudinal,
        false,
        Some(episode_budget),
        references,
    )
    .await
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
