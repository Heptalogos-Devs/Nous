// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::schema::SchemaView;
use super::*;
use nous_runtime::QuerySource;

pub enum CognitionContent {
    Memory(MemoryView),
    Schema(SchemaView),
    Episode(EpisodeView),
    Journal(JournalView),
    Tag {
        tag: TagId,
        revision: Uuid,
        text: String,
    },
}
impl CognitionContent {
    pub(super) fn reference(&self) -> CognitiveRef {
        match self {
            Self::Memory(v) => CognitiveRef::MemoryRevision(v.revision.memory_revision_id),
            Self::Schema(v) => CognitiveRef::CognitiveSchemaRevision(v.revision.schema_revision_id),
            Self::Episode(v) => CognitiveRef::EpisodeRevision(v.revision.episode_revision_id),
            Self::Journal(v) => CognitiveRef::JournalRevision(v.revision.journal_revision_id),
            Self::Tag { tag, .. } => CognitiveRef::Tag(*tag),
        }
    }
    pub(super) fn require_available(
        &self,
        effort: CognitiveEffort,
        explicit: bool,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<()> {
        let reference = self.reference();
        let states = match self {
            Self::Memory(v) => (
                v.object.acceptance_state,
                v.object.integrity_state,
                v.object.suppression_state,
            ),
            Self::Schema(v) => (
                v.schema.acceptance_state,
                v.schema.integrity_state,
                v.schema.suppression_state,
            ),
            Self::Episode(v) => (
                v.object.acceptance_state,
                v.object.integrity_state,
                v.object.suppression_state,
            ),
            Self::Journal(v) => (
                v.object.acceptance_state,
                v.object.integrity_state,
                v.object.suppression_state,
            ),
            Self::Tag { .. } => return Ok(()),
        };
        let states = if let Some(state) = historical.and_then(|v| v.cognition_for(&reference)) {
            (
                serde_json::from_value(state.state["acceptance_state"].clone())
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
                serde_json::from_value(state.state["integrity_state"].clone())
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
                serde_json::from_value(state.state["suppression_state"].clone())
                    .map_err(|e| Error::Infrastructure(e.to_string()))?,
            )
        } else {
            states
        };
        require_context_states(&reference, states)?;
        if let Self::Memory(v) = self
            && !accessibility_eligible(
                v.accessibility_level,
                effort,
                explicit || historical.is_some(),
            )
        {
            return Err(Error::Unavailable(
                "Memory accessibility requires deeper or explicit recall".into(),
            ));
        }
        Ok(())
    }
    fn descriptor(&self) -> QueryDescriptor {
        if let Self::Tag { text, .. } = self {
            return QueryDescriptor {
                reference: self.reference(),
                text: text.clone(),
            };
        }

        let (title, text) = match self {
            Self::Memory(v) => (
                v.revision.title.as_deref(),
                v.revision.representation_text.clone(),
            ),
            Self::Schema(v) => (
                v.revision.title.as_deref(),
                format!(
                    "{} Scope: {} Boundary: {}",
                    v.revision.structural_claim,
                    v.revision.applicability_scope.description,
                    v.revision.boundary_definition
                ),
            ),
            Self::Episode(v) => (
                v.revision.title.as_deref(),
                v.revision.boundary_explanation.clone(),
            ),
            Self::Journal(v) => (v.revision.title.as_deref(), v.revision.narrative.clone()),
            Self::Tag { text, .. } => (None, text.clone()),
        };
        QueryDescriptor {
            reference: self.reference(),
            text: title
                .map_or(text.clone(), |title| format!("{title}: {text}"))
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
        sources: &[QuerySource],
        effort: CognitiveEffort,
        now: DateTime<Utc>,
        configuration: &nous_configuration::ConfigSnapshot,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<QueryDescriptor>> {
        let policy = resolve_accessibility_policy(configuration)?;
        let refs = sources
            .iter()
            .map(|s| s.reference.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let contents = self
            .cognition_contents(subject, &refs, &policy, historical, now)
            .await?;
        contents
            .into_iter()
            .filter_map(|content| {
                let reference = content.reference();
                let explicit = sources.iter().any(|source| {
                    source.reference == reference
                        && (!source.is_runtime() || source.origin == "work_context")
                });
                match content.require_available(effort, explicit, historical) {
                    Ok(()) => Some(Ok(content.descriptor())),
                    Err(Error::Unavailable(_)) => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .collect()
    }

    /// One bounded owner read owns head selection, canonical identity, content and support.
    pub(super) async fn cognition_contents(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
        policy: &AccessibilityPolicy,
        historical: Option<&HistoricalAuthoritySnapshot>,
        now: DateTime<Utc>,
    ) -> Result<Vec<CognitionContent>> {
        let mut tx = self.store.read_view().await?;
        let contents = self
            .cognition_contents_in(&mut tx, subject, refs, policy, historical, now, false)
            .await?;
        tx.commit().await.map_err(db)?;
        Ok(contents)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn cognition_contents_in(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        refs: &[CognitiveRef],
        policy: &AccessibilityPolicy,
        historical: Option<&HistoricalAuthoritySnapshot>,
        now: DateTime<Utc>,
        full_details: bool,
    ) -> Result<Vec<CognitionContent>> {
        if refs.len() > 256 || historical.is_some_and(|v| v.subject != subject) {
            return Err(Error::Invalid("cognition content scope exceeded".into()));
        }
        let mut memory = (vec![], vec![]);
        let mut schema = (vec![], vec![]);
        let mut episode = (vec![], vec![]);
        let mut journal = (vec![], vec![]);
        let mut tags = vec![];
        for reference in refs {
            match reference {
                CognitiveRef::Memory(id) => memory.1.push(id.0),
                CognitiveRef::MemoryRevision(id) => memory.0.push(id.0),
                CognitiveRef::CognitiveSchema(id) => schema.1.push(id.0),
                CognitiveRef::CognitiveSchemaRevision(id) => schema.0.push(id.0),
                CognitiveRef::Episode(id) => episode.1.push(id.0),
                CognitiveRef::EpisodeRevision(id) => episode.0.push(id.0),
                CognitiveRef::Journal(id) => journal.1.push(id.0),
                CognitiveRef::JournalRevision(id) => journal.0.push(id.0),
                CognitiveRef::Tag(id) => tags.push(*id),
                _ => {
                    return Err(Error::Invalid(
                        "content requires a cognition reference".into(),
                    ));
                }
            }
        }
        let mut contents = vec![];
        if !memory.0.is_empty() || !memory.1.is_empty() {
            contents.extend(
                self.memory_views_in(
                    tx,
                    subject,
                    (&memory.0, &memory.1),
                    policy,
                    historical,
                    full_details,
                    now,
                )
                .await?
                .into_values()
                .map(CognitionContent::Memory),
            );
        }
        if !schema.0.is_empty() || !schema.1.is_empty() {
            contents.extend(
                self.schema_views_in(tx, subject, &schema.0, &schema.1)
                    .await?
                    .into_values()
                    .map(CognitionContent::Schema),
            );
        }
        if !episode.0.is_empty() || !episode.1.is_empty() {
            contents.extend(
                self.episode_views_in(tx, subject, &episode.0, &episode.1)
                    .await?
                    .into_values()
                    .map(CognitionContent::Episode),
            );
        }
        if !journal.0.is_empty() || !journal.1.is_empty() {
            contents.extend(
                self.journal_views_in(tx, subject, &journal.0, &journal.1)
                    .await?
                    .into_values()
                    .map(CognitionContent::Journal),
            );
        }
        if let Some(historical) = historical {
            for tag in tags {
                if let Some(canonical) = historical.canonical_tag(tag)
                    && let Some(state) = historical.tags.iter().find(|v| v.tag == canonical)
                {
                    contents.push(CognitionContent::Tag {
                        tag: canonical,
                        revision: state.revision_id,
                        text: state.semantic.text.clone(),
                    });
                }
            }
        } else if !tags.is_empty() {
            let rows = sqlx::query("SELECT DISTINCT t.tag_id,r.tag_revision_id,r.label,r.description,r.kind_hint FROM unnest($2::uuid[]) requested JOIN tags t ON t.tag_id=canonical_tag($1,requested) JOIN tag_revisions r ON r.tag_revision_id=t.current_revision_id WHERE t.subject_id=$1 AND t.status='active'")
                .bind(subject.0).bind(tags.iter().map(|id| id.0).collect::<Vec<_>>()).fetch_all(&mut **tx).await.map_err(db)?;
            for row in rows {
                contents.push(CognitionContent::Tag {
                    tag: TagId(row.try_get("tag_id").map_err(db)?),
                    revision: row.try_get("tag_revision_id").map_err(db)?,
                    text: tag_semantic_representation(
                        &row.try_get::<String, _>("label").map_err(db)?,
                        row.try_get::<Option<String>, _>("description")
                            .map_err(db)?
                            .as_deref(),
                        row.try_get::<Option<String>, _>("kind_hint")
                            .map_err(db)?
                            .as_deref(),
                    )?
                    .text,
                });
            }
        }
        Ok(contents)
    }
}

fn require_context_states(
    source_revision: &CognitiveRef,
    states: (AcceptanceState, IntegrityState, SuppressionState),
) -> Result<()> {
    let name = match source_revision {
        CognitiveRef::MemoryRevision(_) => "Memory",
        CognitiveRef::CognitiveSchemaRevision(_) => "CognitiveSchema",
        CognitiveRef::EpisodeRevision(_) => "Episode",
        CognitiveRef::JournalRevision(_) => "Journal",
        _ => unreachable!(),
    };
    if states.0 != AcceptanceState::Accepted {
        return Err(Error::Unavailable(format!("{name} is withdrawn")));
    }
    if states.1 != IntegrityState::Valid {
        return Err(Error::Unavailable(format!("{name} requires revalidation")));
    }
    if states.2 != SuppressionState::Normal {
        return Err(Error::Unavailable(format!("{name} is suppressed")));
    }
    Ok(())
}
