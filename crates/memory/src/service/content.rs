//! Bounded cognition text reads; complete views and exact support have separate readers.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::{HashMap, HashSet};

pub(super) struct CognitionText {
    pub reference: CognitiveRef,
    pub revision: Uuid,
    pub title: Option<String>,
    pub text: String,
    pub scope: Option<String>,
    pub boundary: Option<String>,
    pub complete: bool,
}
impl CognitionText {
    pub(super) fn descriptor(self) -> QueryDescriptor {
        let text = match (self.scope, self.boundary) {
            (Some(scope), Some(boundary)) => {
                format!("{} Scope: {scope} Boundary: {boundary}", self.text)
            }
            _ => self.text,
        };
        QueryDescriptor {
            reference: self.reference,
            text: self
                .title
                .map_or_else(|| text.clone(), |title| format!("{title}: {text}"))
                .chars()
                .take(2048)
                .collect(),
        }
    }
}

impl MemoryService {
    pub fn owns_content(reference: &CognitiveRef) -> bool {
        matches!(
            reference,
            CognitiveRef::Memory(_)
                | CognitiveRef::MemoryRevision(_)
                | CognitiveRef::CognitiveSchema(_)
                | CognitiveRef::CognitiveSchemaRevision(_)
                | CognitiveRef::Episode(_)
                | CognitiveRef::EpisodeRevision(_)
                | CognitiveRef::Journal(_)
                | CognitiveRef::JournalRevision(_)
                | CognitiveRef::Tag(_)
        )
    }

    pub async fn query_descriptors(
        &self,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        effort: CognitiveEffort,
        now: DateTime<Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
        source_read: Option<&SourceReadView>,
    ) -> Result<Vec<QueryDescriptor>> {
        let policy = resolve_accessibility_policy(configuration)?;
        let mut tx = self.store.read_view().await?;
        let mut admitted = Vec::new();
        for r in references {
            if source_read.is_none()
                || AuthorityStore::source_read_allowed_in(
                    &mut tx,
                    source_read.expect("checked read"),
                    &r.0,
                )
                .await?
            {
                admitted.push(r.clone());
            }
        }
        let references = admitted.as_slice();
        let contents = self
            .content_in(
                &mut tx, subject, references, &policy, historical, now, effort, 2048,
            )
            .await?;
        tx.commit().await.map_err(db)?;
        let mut seen = HashSet::new();
        contents
            .into_iter()
            .filter_map(|content| match content {
                Ok(content) if seen.insert(content.reference.clone()) => {
                    Some(Ok(content.descriptor()))
                }
                Ok(_) | Err(Error::Unavailable(_) | Error::NotFound(_)) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn content_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        references: &[(CognitiveRef, bool)],
        policy: &AccessibilityPolicy,
        historical: Option<&HistoricalAuthoritySnapshot>,
        now: DateTime<Utc>,
        effort: CognitiveEffort,
        max_bytes: usize,
    ) -> Result<Vec<Result<CognitionText>>> {
        if references.len() > 256 || historical.is_some_and(|v| v.subject != subject) {
            return Err(Error::Invalid("cognition content scope exceeded".into()));
        }
        let selected = references
            .iter()
            .map(|(reference, _)| {
                historical
                    .and_then(|view| view.cognition_for(reference))
                    .filter(|state| state.object == *reference)
                    .map_or_else(|| reference.clone(), |state| state.head.clone())
            })
            .collect::<Vec<_>>();
        let (kinds, values): (Vec<_>, Vec<_>) = selected.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) WITH ORDINALITY r(kind,value,ordinal))
SELECT q.ordinal,'memory_revision' kind,r.memory_revision_id id,o.memory_id object_id,
 o.acceptance_state,o.integrity_state,o.suppression_state,
 CASE WHEN $4>0 THEN r.title END title,CASE WHEN $4>0 THEN left(r.representation_text,$4) ELSE '' END text,NULL::text scope,NULL::text boundary,
 octet_length(r.representation_text)<=$4 complete
 FROM requested q JOIN memory_objects o ON o.subject_id=$1
 JOIN memory_revisions r ON r.memory_id=o.memory_id
 WHERE (q.kind='memory_revision' AND r.memory_revision_id::text=q.value)
    OR (q.kind='memory' AND o.memory_id::text=q.value AND r.memory_revision_id=o.current_revision_id)
UNION ALL
SELECT q.ordinal,'cognitive_schema_revision',r.schema_revision_id,o.schema_id,
 o.acceptance_state,o.integrity_state,o.suppression_state,CASE WHEN $4>0 THEN r.title END,
 CASE WHEN $4>0 THEN left(r.structural_claim,$4) ELSE '' END,CASE WHEN $4>0 THEN left(r.applicability_description,$4) ELSE '' END,CASE WHEN $4>0 THEN left(r.boundary_definition,$4) ELSE '' END,
 octet_length(r.structural_claim)+octet_length(r.applicability_description)+octet_length(r.boundary_definition)+2<=$4
 FROM requested q JOIN cognitive_schemas o ON o.subject_id=$1
 JOIN cognitive_schema_revisions r ON r.schema_id=o.schema_id
 WHERE (q.kind='cognitive_schema_revision' AND r.schema_revision_id::text=q.value)
    OR (q.kind='cognitive_schema' AND o.schema_id::text=q.value AND r.schema_revision_id=o.current_revision_id)
UNION ALL
SELECT q.ordinal,'episode_revision',r.episode_revision_id,o.episode_id,
 o.acceptance_state,o.integrity_state,o.suppression_state,CASE WHEN $4>0 THEN r.title END,
 CASE WHEN $4>0 THEN left(r.boundary_explanation,$4) ELSE '' END,NULL,NULL,octet_length(r.boundary_explanation)<=$4
 FROM requested q JOIN episode_objects o ON o.subject_id=$1 JOIN episode_revisions r ON r.episode_id=o.episode_id
 WHERE (q.kind='episode_revision' AND r.episode_revision_id::text=q.value)
    OR (q.kind='episode' AND o.episode_id::text=q.value AND r.episode_revision_id=o.current_revision_id)
UNION ALL
SELECT q.ordinal,'journal_revision',r.journal_revision_id,o.journal_id,
 o.acceptance_state,o.integrity_state,o.suppression_state,CASE WHEN $4>0 THEN r.title END,
 CASE WHEN $4>0 THEN left(r.narrative,$4) ELSE '' END,NULL,NULL,octet_length(r.narrative)<=$4
 FROM requested q JOIN journal_objects o ON o.subject_id=$1 JOIN journal_revisions r ON r.journal_id=o.journal_id
 WHERE (q.kind='journal_revision' AND r.journal_revision_id::text=q.value)
    OR (q.kind='journal' AND o.journal_id::text=q.value AND r.journal_revision_id=o.current_revision_id)
UNION ALL
SELECT q.ordinal,'tag',t.tag_id,r.tag_revision_id,'accepted','valid','normal',
 CASE WHEN $4>0 THEN r.label END,CASE WHEN $4>0 THEN COALESCE(r.description,'') ELSE '' END,
 CASE WHEN $4>0 THEN r.kind_hint END,NULL,octet_length(r.label)+octet_length(COALESCE(r.description,''))+octet_length(COALESCE(r.kind_hint,''))+32<=$4
 FROM requested q JOIN tags t ON q.kind='tag' AND t.tag_id=canonical_tag($1,CASE WHEN q.kind='tag' THEN q.value::uuid END)
 JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.status='active'
"#).bind(subject.0).bind(kinds).bind(values).bind(i32::try_from(max_bytes).map_err(|_| Error::Invalid("content bound exceeded".into()))?)
            .fetch_all(&mut **tx).await.map_err(db)?;
        let memories = rows
            .iter()
            .filter(|r| r.get::<String, _>("kind") == "memory_revision")
            .map(|r| r.try_get::<Uuid, _>("object_id").map_err(db))
            .collect::<Result<Vec<_>>>()?;
        let accessibility = self
            .memory_accessibility_in(tx, subject, &memories, policy, historical, now)
            .await?;
        let mut found = HashMap::new();
        for row in rows {
            let ordinal: i64 = row.try_get("ordinal").map_err(db)?;
            found.insert(ordinal as usize - 1, row);
        }
        Ok(references
            .iter()
            .enumerate()
            .map(|(ordinal, (requested, explicit))| {
                read_content(
                    requested,
                    *explicit,
                    found.remove(&ordinal),
                    historical,
                    &accessibility,
                    effort,
                    max_bytes,
                )
            })
            .collect())
    }
}

fn read_content(
    requested: &CognitiveRef,
    explicit: bool,
    row: Option<sqlx::postgres::PgRow>,
    historical: Option<&HistoricalAuthoritySnapshot>,
    accessibility: &HashMap<Uuid, AccessibilityLevel>,
    effort: CognitiveEffort,
    max_bytes: usize,
) -> Result<CognitionText> {
    if !MemoryService::owns_content(requested) {
        return Err(Error::Invalid(
            "content requires a cognition reference".into(),
        ));
    }
    if let (Some(view), CognitiveRef::Tag(tag)) = (historical, requested) {
        let canonical = view
            .canonical_tag(*tag)
            .ok_or_else(|| Error::Unavailable("historical Tag is unavailable".into()))?;
        let state = view
            .tag_state(canonical)
            .ok_or_else(|| Error::NotFound("historical Tag not found".into()))?;
        return Ok(CognitionText {
            reference: CognitiveRef::Tag(canonical),
            revision: state.revision_id,
            title: None,
            text: if max_bytes > 0 {
                state.semantic.text.clone()
            } else {
                String::new()
            },
            scope: None,
            boundary: None,
            complete: true,
        });
    }
    let row =
        row.ok_or_else(|| Error::NotFound(format!("{requested}: cognition source not found")))?;
    let kind: String = row.try_get("kind").map_err(db)?;
    let id: Uuid = row.try_get("id").map_err(db)?;
    let reference = parse_reference(&kind, &id.to_string())?;
    let state = historical.and_then(|v| v.cognition_for(&reference));
    if historical.is_some() && state.is_none() {
        return Err(Error::NotFound(
            "cognition source is outside the captured view".into(),
        ));
    }
    if kind != "tag" {
        for (field, expected, reason) in [
            ("acceptance_state", "accepted", "is withdrawn"),
            ("integrity_state", "valid", "requires revalidation"),
            ("suppression_state", "normal", "is suppressed"),
        ] {
            let actual: String = row.try_get(field).map_err(db)?;
            if state.map_or(actual.as_str(), |v| v.state[field].as_str().unwrap_or("")) != expected
            {
                let name = match reference {
                    CognitiveRef::MemoryRevision(_) => "Memory",
                    CognitiveRef::CognitiveSchemaRevision(_) => "CognitiveSchema",
                    CognitiveRef::EpisodeRevision(_) => "Episode",
                    CognitiveRef::JournalRevision(_) => "Journal",
                    _ => unreachable!("owned cognition family"),
                };
                return Err(Error::Unavailable(format!("{name} {reason}")));
            }
        }
    }
    let owner: Uuid = row.try_get("object_id").map_err(db)?;
    if kind == "memory_revision"
        && !accessibility_eligible(
            accessibility[&owner],
            effort,
            explicit || historical.is_some(),
        )
    {
        return Err(Error::Unavailable(
            "Memory accessibility requires deeper or explicit recall".into(),
        ));
    }
    let mut title: Option<String> = row.try_get("title").map_err(db)?;
    let mut text: String = row.try_get("text").map_err(db)?;
    let mut scope: Option<String> = row.try_get("scope").map_err(db)?;
    if kind == "tag" && max_bytes > 0 {
        text = tag_semantic_representation(
            title.as_deref().unwrap_or_default(),
            Some(&text),
            scope.as_deref(),
        )?
        .text;
        title = None;
        scope = None;
    }
    Ok(CognitionText {
        reference,
        revision: if kind == "tag" { owner } else { id },
        title,
        text,
        scope,
        boundary: row.try_get("boundary").map_err(db)?,
        complete: row.try_get("complete").map_err(db)?,
    })
}
