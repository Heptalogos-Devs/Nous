//! Shared current/as-of document rows, selected before body extraction.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::*;
use nous_core::*;
use sqlx::{Postgres, Row, Transaction};
use std::collections::HashMap;

pub(crate) async fn historical_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    view: &HistoricalAuthoritySnapshot,
    budget: EpisodeTextBudget,
    memory_allowed: bool,
    requested: Option<&[CognitiveRef]>,
) -> Result<(Vec<TextProjectionSource>, Vec<ConceptProjectionTag>)> {
    let mut sources = historical_catalog_sources_in(tx, view, memory_allowed, requested).await?;
    let tags = historical_tag_sources(view, memory_allowed, requested, &mut sources);
    enrich_cognition_sources_in(tx, view, memory_allowed, &mut sources).await?;
    episode_fragments_in(tx, view, budget, &mut sources).await?;
    sources.sort_by_key(|source| source.reference.to_string());
    Ok((sources, tags))
}

async fn historical_catalog_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    view: &HistoricalAuthoritySnapshot,
    memory_allowed: bool,
    requested: Option<&[CognitiveRef]>,
) -> Result<Vec<TextProjectionSource>> {
    // Selection is frozen at the owner level. Current purge is a hard fence,
    // while current head, suppression and canonical Tag never select this corpus.
    let selected: Vec<_> = match requested {
        Some(references) => references
            .iter()
            .filter(|reference| {
                memory_allowed
                    && view
                        .selected_cognition_for(reference)
                        .is_some_and(|state| eligible_state(&state.state))
            })
            .cloned()
            .collect(),
        None => view
            .cognition
            .iter()
            .filter(|state| memory_allowed && eligible_state(&state.state))
            .flat_map(|state| state.revisions.iter().cloned())
            .collect(),
    };
    let material: Vec<_> = match requested {
        Some(references) => references
            .iter()
            .filter(|reference| view.material_document_contains(reference))
            .cloned()
            .collect(),
        None => view.material_documents.clone(),
    };
    let (kinds, values): (Vec<_>, Vec<_>) = selected
        .iter()
        .chain(material.iter())
        .map(reference_parts)
        .unzip();
    let rows = sqlx::query(r#"
WITH selected AS (SELECT * FROM unnest($2::text[],$3::text[]) r(kind,value)), catalog AS (
 SELECT s.kind,s.value,r.title,r.representation_text text,NULL::text source_class FROM selected s JOIN memory_revisions r ON s.kind='memory_revision' AND r.memory_revision_id=CASE WHEN s.kind='memory_revision' THEN s.value::uuid END JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND o.purge_state='normal'
 UNION ALL SELECT s.kind,s.value,r.title,r.structural_claim,NULL FROM selected s JOIN cognitive_schema_revisions r ON s.kind='cognitive_schema_revision' AND r.schema_revision_id=CASE WHEN s.kind='cognitive_schema_revision' THEN s.value::uuid END JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND o.purge_state='normal'
 UNION ALL SELECT s.kind,s.value,r.title,NULL,NULL FROM selected s JOIN episode_revisions r ON s.kind='episode_revision' AND r.episode_revision_id=CASE WHEN s.kind='episode_revision' THEN s.value::uuid END JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND o.purge_state='normal'
 UNION ALL SELECT s.kind,s.value,r.title,COALESCE(r.title,'')||E'\n'||r.narrative||E'\n'||COALESCE((SELECT string_agg(text,E'\n' ORDER BY ordinal) FROM journal_revision_points WHERE journal_revision_id=r.journal_revision_id),''),NULL FROM selected s JOIN journal_revisions r ON s.kind='journal_revision' AND r.journal_revision_id=CASE WHEN s.kind='journal_revision' THEN s.value::uuid END JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND o.purge_state='normal'
 UNION ALL SELECT s.kind,s.value,NULL,NULL,o.source_class FROM selected s JOIN observation_occurrences o ON s.kind='occurrence' AND o.occurrence_id=CASE WHEN s.kind='occurrence' THEN s.value::uuid END LEFT JOIN artifacts a USING(artifact_id) WHERE o.subject_id=$1
 UNION ALL SELECT s.kind,s.value,NULL,NULL,NULL FROM selected s JOIN source_regions r ON s.kind='source_region' AND r.source_region_id=CASE WHEN s.kind='source_region' THEN s.value::uuid END JOIN artifacts a USING(artifact_id) WHERE r.subject_id=$1
 UNION ALL SELECT s.kind,s.value,d.representation_kind,NULL,'derived' FROM selected s JOIN derived_representations d ON s.kind='derived_representation' AND d.derived_representation_id=CASE WHEN s.kind='derived_representation' THEN s.value::uuid END WHERE d.subject_id=$1
 UNION ALL SELECT s.kind,s.value,d.representation_kind,NULL,'derived' FROM selected s JOIN derived_regions r ON s.kind='derived_region' AND r.derived_region_id=CASE WHEN s.kind='derived_region' THEN s.value::uuid END JOIN derived_representations d USING(derived_representation_id) WHERE r.subject_id=$1
) SELECT * FROM catalog ORDER BY kind,value
"#).bind(view.subject.0).bind(kinds).bind(values).fetch_all(&mut **tx).await.map_err(database_error)?;
    let mut sources = Vec::new();
    for row in rows {
        let reference = parse_reference(
            &row.get::<String, _>("kind"),
            &row.get::<String, _>("value"),
        )?;
        sources.push(TextProjectionSource {
            reference: reference.clone(),
            text: row.get("text"),
            title: row.get("title"),
            source_class: row.get("source_class"),
            member_fragments: vec![],
            source_region: match reference {
                CognitiveRef::SourceRegion(id) => Some(id),
                _ => None,
            },
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
    let rows = sqlx::query("SELECT episode_revision_id,title,boundary_explanation,experience_time_kind,experience_time_start,experience_time_end FROM episode_revisions WHERE subject_id=$1 AND episode_revision_id=ANY($2::uuid[])")
            .bind(view.subject.0).bind(&episode_ids).fetch_all(&mut **tx).await.map_err(database_error)?;
    for row in rows {
        let reference =
            CognitiveRef::EpisodeRevision(EpisodeRevisionId(row.get("episode_revision_id")));
        if let Some(source) = sources
            .iter_mut()
            .find(|source| source.reference == reference)
        {
            source.text = Some(crate::projection::text::episode_representation(&row));
        }
    }
    for source in &mut sources {
        if matches!(source.reference, CognitiveRef::JournalRevision(_))
            && let Some(text) = &mut source.text
        {
            crate::projection::text::truncate_text(text, 65536);
        }
    }
    Ok(sources)
}

fn historical_tag_sources(
    view: &HistoricalAuthoritySnapshot,
    memory_allowed: bool,
    requested: Option<&[CognitiveRef]>,
    sources: &mut Vec<TextProjectionSource>,
) -> Vec<ConceptProjectionTag> {
    let tag_states: Vec<_> = match requested {
        Some(references) => references
            .iter()
            .filter_map(|reference| match reference {
                CognitiveRef::Tag(tag) => view.tag_state(*tag),
                _ => None,
            })
            .collect(),
        None => view.tags.iter().collect(),
    };
    let tags: Vec<_> = tag_states
        .into_iter()
        .filter(|tag| memory_allowed && tag.status == "active")
        .map(|tag| ConceptProjectionTag {
            tag: tag.tag,
            revision: tag.revision_id,
            semantic: tag.semantic.clone(),
            attachments: vec![],
        })
        .collect();
    for tag in &tags {
        sources.push(TextProjectionSource {
            reference: CognitiveRef::Tag(tag.tag),
            text: Some(tag.semantic.text.clone()),
            member_fragments: vec![],
            title: None,
            source_class: None,
            source_region: None,
            entity_refs: vec![],
            tag_ids: vec![],
            schema_ids: vec![],
        });
    }
    tags
}

async fn enrich_cognition_sources_in(
    tx: &mut Transaction<'_, Postgres>,
    view: &HistoricalAuthoritySnapshot,
    memory_allowed: bool,
    sources: &mut [TextProjectionSource],
) -> Result<()> {
    let indices: HashMap<_, _> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| (s.reference.clone(), i))
        .collect();
    let memory_ids: Vec<_> = sources
        .iter()
        .filter_map(|s| match s.reference {
            CognitiveRef::MemoryRevision(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let schema_ids: Vec<_> = sources
        .iter()
        .filter_map(|s| match s.reference {
            CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let rows = sqlx::query(r#"
 SELECT 'memory_revision' kind,t.memory_revision_id::text value,t.tag_id FROM memory_revision_tags t WHERE t.memory_revision_id=ANY($1::uuid[])
 UNION ALL SELECT 'cognitive_schema_revision',r.schema_revision_id::text,unnest(r.tags) FROM cognitive_schema_revisions r WHERE r.schema_revision_id=ANY($2::uuid[])
"#).bind(&memory_ids).bind(schema_ids).fetch_all(&mut **tx).await.map_err(database_error)?;
    for row in rows {
        let reference = parse_reference(
            &row.get::<String, _>("kind"),
            &row.get::<String, _>("value"),
        )?;
        if let Some(tag) = view.canonical_tag(TagId(row.get("tag_id")))
            && let Some(&index) = indices.get(&reference)
        {
            sources[index].tag_ids.push(tag.0.to_string());
        }
    }
    let rows = sqlx::query("SELECT memory_revision_id,entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=ANY($1::uuid[]) ORDER BY memory_revision_id,entity_ref")
            .bind(memory_ids).fetch_all(&mut **tx).await.map_err(database_error)?;
    for row in rows {
        if let Some(&index) = indices.get(&CognitiveRef::MemoryRevision(MemoryRevisionId(
            row.get("memory_revision_id"),
        ))) {
            sources[index].entity_refs.push(row.get("entity_ref"));
        }
    }
    let bindings: Vec<_> = view.entity_bindings.to_vec();
    let occurrences: Vec<_> = sources
        .iter()
        .filter_map(|s| match s.reference {
            CognitiveRef::Occurrence(id) => Some(id.0),
            _ => None,
        })
        .collect();
    let rows = sqlx::query("SELECT m.occurrence_id,b.entity_ref FROM entity_binding_revisions b JOIN entity_mentions m USING(mention_id) WHERE b.binding_revision_id=ANY($1::uuid[]) AND b.binding_state='bound' AND b.entity_ref IS NOT NULL AND m.occurrence_id=ANY($2::uuid[]) ORDER BY m.occurrence_id,b.entity_ref")
            .bind(bindings).bind(occurrences).fetch_all(&mut **tx).await.map_err(database_error)?;
    for row in rows {
        if let Some(&index) = indices.get(&CognitiveRef::Occurrence(OccurrenceId(
            row.get("occurrence_id"),
        ))) {
            sources[index].entity_refs.push(row.get("entity_ref"));
        }
    }
    let cognition_keys: Vec<_> = sources
        .iter()
        .filter(|source| {
            matches!(
                source.reference,
                CognitiveRef::MemoryRevision(_)
                    | CognitiveRef::EpisodeRevision(_)
                    | CognitiveRef::JournalRevision(_)
                    | CognitiveRef::CognitiveSchemaRevision(_)
            )
        })
        .map(|source| reference_parts(&source.reference))
        .collect();
    let (basis_kinds, basis_values): (Vec<_>, Vec<_>) = cognition_keys.into_iter().unzip();
    let links = if memory_allowed && !basis_kinds.is_empty() {
        view.schema_evidence_links.clone()
    } else {
        vec![]
    };
    let rows = sqlx::query("SELECT schema_revision_id,basis_kind,basis_ref FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND link_id=ANY($2::uuid[]) AND role='support' AND epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND basis_kind<>'evidence' AND (basis_kind,basis_ref) IN (SELECT * FROM unnest($3::text[],$4::text[]))")
            .bind(view.subject.0).bind(links).bind(basis_kinds).bind(basis_values).fetch_all(&mut **tx).await.map_err(database_error)?;
    for row in rows {
        let from = CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId(
            row.get("schema_revision_id"),
        ));
        let to = parse_reference(
            &row.get::<String, _>("basis_kind"),
            &row.get::<String, _>("basis_ref"),
        )?;
        if let Some(state) = view.cognition_for(&from).filter(|state| {
            view.selected_cognition_for(&from).is_some() && eligible_state(&state.state)
        }) && let Some(&index) = indices.get(&to)
        {
            sources[index]
                .schema_ids
                .push(reference_parts(&state.object).1);
        }
    }
    Ok(())
}

async fn episode_fragments_in(
    tx: &mut Transaction<'_, Postgres>,
    view: &HistoricalAuthoritySnapshot,
    budget: EpisodeTextBudget,
    sources: &mut [TextProjectionSource],
) -> Result<()> {
    // Episode fragment selection is likewise bounded by the same effective Material set.
    let episode_ids = sources
        .iter()
        .filter_map(|s| match s.reference {
            CognitiveRef::EpisodeRevision(id) => Some(id.0),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut fragments = crate::episode_text::episode_member_text_input_in_view(
        tx,
        view.subject,
        &episode_ids,
        budget,
        Some(view),
    )
    .await?;
    for source in sources {
        if let CognitiveRef::EpisodeRevision(id) = source.reference {
            source.member_fragments = fragments.remove(&id.0).unwrap_or_default();
        }
    }
    Ok(())
}

pub(crate) fn document_references(
    view: &HistoricalAuthoritySnapshot,
    memory_allowed: bool,
) -> Vec<CognitiveRef> {
    view.cognition
        .iter()
        .filter(|state| memory_allowed && eligible_state(&state.state))
        .flat_map(|state| state.revisions.iter().cloned())
        .chain(view.material_documents.iter().cloned())
        .chain(
            view.tags
                .iter()
                .filter(|tag| memory_allowed && tag.status == "active")
                .map(|tag| CognitiveRef::Tag(tag.tag)),
        )
        .collect()
}
fn eligible_state(state: &serde_json::Value) -> bool {
    state["acceptance_state"] == "accepted"
        && state["integrity_state"] == "valid"
        && state["suppression_state"] == "normal"
        && state["purge_state"] == "normal"
}
