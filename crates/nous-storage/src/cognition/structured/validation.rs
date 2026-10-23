use crate::SourceStatus;
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b062082afb75ccfae4c3f69fc606d3e3cc862; see NOTICE.
use super::read::{decode, object};
use crate::codec::*;
use crate::source::closure::resolve_sources;
use crate::{storage_error, write_error};
use nous_domain::*;
use sqlx::{Row, SqliteConnection};
use std::collections::HashSet;

pub(crate) fn dependencies(value: &StructuredObject) -> Vec<CognitiveRef> {
    let mut value = value.clone();
    match &mut value.body {
        CognitiveContent::Concept(content) => {
            content.canonical = None;
            content.lineage.clear();
        }
        CognitiveContent::Episode(content) => content.lineage.clear(),
        CognitiveContent::Journal(content) => content.lineage.clear(),
        CognitiveContent::Schema(content) => content.lineage.clear(),
        _ => {}
    }
    value.references()
}

pub(crate) async fn reject_cycle(
    conn: &mut SqliteConnection,
    owner: &StructuredObject,
) -> Result<()> {
    let mut stack = dependencies(owner);
    let mut seen = HashSet::new();
    while let Some(target) = stack.pop() {
        if target.id == owner.id.as_str() && target.kind == owner.kind() {
            return Err(Error::invalid(
                "cognition dependency would create an object cycle",
            ));
        }
        if !seen.insert((enum_name(&target.kind)?, target.id.clone(), target.revision)) {
            continue;
        }
        if seen.len() > 1024 {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "cognition dependency closure exceeds 1024 revisions",
            ));
        }
        if target.kind == CognitiveKind::Memory {
            continue;
        }
        let nested = object(
            conn,
            &owner.namespace_id,
            &CognitiveId(target.id),
            Some(target.revision),
        )
        .await?;
        stack.extend(dependencies(&nested));
    }
    Ok(())
}

pub(crate) async fn validate_content_in(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
    _previous: Option<&StructuredObject>,
) -> Result<()> {
    match &value.body {
        CognitiveContent::MemorySemantics(content) => {
            let memory=sqlx::query("SELECT material_id FROM memory_revisions WHERE namespace_id=? AND subject_id=? AND memory_id=? AND revision=?")
                .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(content.memory_id.as_str()).bind(content.memory_revision)
                .fetch_optional(&mut *conn).await.map_err(storage_error)?;
            if let (Some(memory), Some(grounding)) = (memory, &content.grounding_observation) {
                let needs_contact =
                    content.cognitive_role != nous_domain::CognitiveRole::Declarative;
                let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM observations WHERE namespace_id=? AND subject_id=? AND id=? AND material_id=? AND (?=0 OR kind='contact'))")
                    .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(grounding.as_str()).bind(memory.get::<String,_>("material_id"))
                    .bind(needs_contact).fetch_one(&mut *conn).await.map_err(storage_error)?;
                if !valid {
                    return Err(Error::invalid(
                        "Memory semantics grounding must match its source material; experiential roles require Contact",
                    ));
                }
            }
            if content.formation_mode == nous_domain::FormationMode::Synthesized {
                let sources = value
                    .evidence_links()
                    .into_iter()
                    .filter(|link| link.polarity == nous_domain::EvidencePolarity::Support)
                    .map(|link| link.source)
                    .collect();
                require_independent_roots(conn, value, sources).await?;
            }
        }
        CognitiveContent::Concept(content) => {
            let mut target = content.canonical.clone();
            let mut seen = HashSet::from([value.id.to_string()]);
            while let Some(canonical) = target {
                if !seen.insert(canonical.id.clone()) {
                    return Err(Error::invalid(
                        "Concept canonical redirects contain an identity cycle",
                    ));
                }
                if seen.len() > 64 {
                    return Err(Error::new(
                        ErrorCode::ResourceExhausted,
                        "Concept canonical chain exceeds 64 identities",
                    ));
                }
                let current =
                    object(conn, &value.namespace_id, &CognitiveId(canonical.id), None).await?;
                let CognitiveContent::Concept(current) = current.body else {
                    return Err(Error::invalid("canonical target must be a Concept"));
                };
                target = current.canonical;
            }
        }
        CognitiveContent::Episode(content) if !value.deleted => {
            if let Some(parent) = &content.parent {
                let parent = object(
                    conn,
                    &value.namespace_id,
                    &CognitiveId(parent.id.clone()),
                    Some(parent.revision),
                )
                .await?;
                let CognitiveContent::Episode(parent) = parent.body else {
                    return Err(Error::invalid("Episode parent must be an Episode"));
                };
                if parent.track != content.track
                    || !contains_range(&parent.experience, &content.experience)
                {
                    return Err(Error::invalid(
                        "Episode child must share parent track and lie inside its experience span",
                    ));
                }
            }
            let rows=sqlx::query("SELECT r.* FROM structured_objects o JOIN structured_revisions r ON r.object_id=o.id AND r.revision=o.current_revision WHERE o.namespace_id=? AND o.subject_id=? AND o.kind='episode' AND r.deleted=0 AND o.id<>?")
                .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(value.id.as_str()).fetch_all(&mut *conn).await.map_err(storage_error)?;
            for row in rows {
                let sibling = decode(conn, row).await?;
                let CognitiveContent::Episode(sibling) = sibling.body else {
                    continue;
                };
                let same_parent = match (&content.parent, &sibling.parent) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.id == b.id && a.revision == b.revision,
                    _ => false,
                };
                if content.track == sibling.track
                    && same_parent
                    && overlaps(&content.experience, &sibling.experience)
                {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "same-track Episode sibling experience spans overlap",
                    ));
                }
            }
        }
        CognitiveContent::Journal(content) => {
            if content.knowledge_cutoff < 0 || content.knowledge_cutoff >= value.commit_seq {
                return Err(Error::invalid(
                    "Journal cutoff must name authority committed before this operation",
                ));
            }
            let (nodes, _) = resolve_sources(
                conn,
                &value.namespace_id,
                &value.subject_id,
                value.sources(),
                false,
            )
            .await?;
            if nodes.iter().any(|node| {
                node.commit_seq
                    .is_none_or(|seq| seq > content.knowledge_cutoff)
            }) {
                return Err(Error::invalid(
                    "Journal source closure lies after its knowledge cutoff or has a gap",
                ));
            }
        }
        CognitiveContent::Schema(content)
            if content.formation_mode == nous_domain::FormationMode::Synthesized =>
        {
            require_independent_roots(conn, value, content.support.clone()).await?;
        }
        _ => {}
    }
    Ok(())
}

pub(crate) async fn require_independent_roots(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
    sources: Vec<EvidenceSource>,
) -> Result<()> {
    let (nodes, roots) =
        resolve_sources(conn, &value.namespace_id, &value.subject_id, sources, true).await?;
    if nodes
        .iter()
        .any(|node| node.status == SourceStatus::Missing)
        || roots
            .iter()
            .filter(|root| root.status == SourceStatus::Available)
            .count()
            < 2
    {
        return Err(Error::invalid(
            "synthesized cognition needs two independent available positive source roots",
        ));
    }
    Ok(())
}

pub(crate) fn contains_range(parent: &TimeRange, child: &TimeRange) -> bool {
    parent.domain == child.domain
        && match (parent.start, parent.end, child.start, child.end) {
            (Some(a), Some(b), Some(c), Some(d)) => a <= c && d <= b,
            _ => false,
        }
}

pub(crate) fn overlaps(a: &TimeRange, b: &TimeRange) -> bool {
    a.domain == b.domain
        && match (a.start, a.end, b.start, b.end) {
            (Some(a), Some(b), Some(c), Some(d)) => a < d && c < b,
            _ => false,
        }
}

pub(crate) async fn sync_aliases(
    conn: &mut SqliteConnection,
    value: &StructuredObject,
) -> Result<()> {
    let CognitiveContent::Concept(content) = &value.body else {
        return Ok(());
    };
    sqlx::query("DELETE FROM concept_aliases WHERE concept_id=?")
        .bind(value.id.as_str())
        .execute(&mut *conn)
        .await
        .map_err(write_error)?;
    if value.deleted || content.canonical.is_some() {
        return Ok(());
    }
    let mut aliases = HashSet::new();
    for alias in std::iter::once(&content.label).chain(&content.aliases) {
        let key = alias.trim().to_lowercase();
        if !aliases.insert(key.clone()) {
            continue;
        }
        sqlx::query("INSERT INTO concept_aliases(namespace_id,subject_id,alias_key,concept_id,revision) VALUES (?,?,?,?,?)")
            .bind(value.namespace_id.as_str()).bind(value.subject_id.as_str()).bind(key).bind(value.id.as_str()).bind(value.revision)
            .execute(&mut *conn).await.map_err(write_error)?;
    }
    Ok(())
}
