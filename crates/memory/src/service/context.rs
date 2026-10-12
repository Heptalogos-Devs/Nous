// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

/// Memory-owned context content after current lifecycle and accessibility checks.
#[derive(Debug)]
pub struct MemoryContextProjection {
    pub source_revision: CognitiveRef,
    pub authority: AuthorityClass,
    pub text: Option<String>,
    pub evidence: Vec<EvidenceHandle>,
}

impl MemoryService {
    pub async fn context_projection(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        policy: &AccessibilityPolicy,
        now: DateTime<Utc>,
        effort: CognitiveEffort,
        explicit: bool,
        max_bytes: usize,
    ) -> Result<MemoryContextProjection> {
        if !matches!(
            reference,
            CognitiveRef::Memory(_) | CognitiveRef::MemoryRevision(_)
        ) {
            let (source, text, basis, states) = match reference {
                CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_) => {
                    let view = match reference {
                        CognitiveRef::CognitiveSchema(id) => self.schema(subject, *id).await?,
                        CognitiveRef::CognitiveSchemaRevision(id) => {
                            self.schema_revision(subject, *id).await?
                        }
                        _ => unreachable!(),
                    };
                    (
                        CognitiveRef::CognitiveSchemaRevision(view.revision.schema_revision_id),
                        format!(
                            "{}\n{}",
                            view.revision.structural_claim,
                            view.revision.applicability_scope.description
                        ),
                        view.evidence_links
                            .into_iter()
                            .map(|link| link.basis)
                            .collect(),
                        (
                            view.schema.acceptance_state,
                            view.schema.integrity_state,
                            view.schema.suppression_state,
                        ),
                    )
                }
                CognitiveRef::Episode(_) | CognitiveRef::EpisodeRevision(_) => {
                    let view = match reference {
                        CognitiveRef::Episode(id) => self.episode(subject, *id, None).await?,
                        CognitiveRef::EpisodeRevision(id) => {
                            self.episode_revision(subject, *id).await?
                        }
                        _ => unreachable!(),
                    };
                    (
                        CognitiveRef::EpisodeRevision(view.revision.episode_revision_id),
                        view.revision.boundary_explanation,
                        view.basis,
                        (
                            view.object.acceptance_state,
                            view.object.integrity_state,
                            view.object.suppression_state,
                        ),
                    )
                }
                CognitiveRef::Journal(_) | CognitiveRef::JournalRevision(_) => {
                    let view = match reference {
                        CognitiveRef::Journal(id) => self.journal(subject, *id, None).await?,
                        CognitiveRef::JournalRevision(id) => {
                            self.journal_revision(subject, *id).await?
                        }
                        _ => unreachable!(),
                    };
                    return journal_context_projection(view, max_bytes);
                }
                _ => {
                    return Err(Error::Invalid(
                        "context requires a cognition reference".into(),
                    ));
                }
            };
            require_context_states(&source, states)?;
            return Ok(project_context(source, text, basis, max_bytes));
        }
        let revision = match reference {
            CognitiveRef::Memory(id) => sqlx::query_scalar::<_, Uuid>(
                "SELECT current_revision_id FROM memory_objects WHERE subject_id=$1 AND memory_id=$2",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("memory not found".into()))?,
            CognitiveRef::MemoryRevision(id) => id.0,
            _ => return Err(Error::Invalid("Memory context requires a Memory reference".into())),
        };
        let memory = self
            .memory_views(subject, &[revision], policy, None, false, now)
            .await?
            .remove(&revision)
            .ok_or_else(|| Error::NotFound("memory not found".into()))?;
        let source = CognitiveRef::MemoryRevision(memory.revision.memory_revision_id);
        require_context_states(
            &source,
            (
                memory.object.acceptance_state,
                memory.object.integrity_state,
                memory.object.suppression_state,
            ),
        )?;
        if !accessibility_eligible(memory.accessibility_level, effort, explicit) {
            return Err(Error::Unavailable(
                "Memory accessibility requires deeper or explicit recall".into(),
            ));
        }
        Ok(project_context(
            source,
            memory.revision.representation_text,
            memory.basis,
            max_bytes,
        ))
    }
}

fn journal_context_projection(
    view: JournalView,
    max_bytes: usize,
) -> Result<MemoryContextProjection> {
    let source = CognitiveRef::JournalRevision(view.revision.journal_revision_id);
    require_context_states(
        &source,
        (
            view.object.acceptance_state,
            view.object.integrity_state,
            view.object.suppression_state,
        ),
    )?;
    let mut text = view.revision.narrative;
    let mut basis = Vec::new();
    for point in view.points {
        text.push('\n');
        text.push_str(&point.text);
        basis.extend(point.basis);
    }
    let mut projection = project_context(source, text, basis, max_bytes);
    projection
        .evidence
        .extend(view.sources.into_iter().map(|reference| EvidenceHandle {
            reference,
            epistemic_relation: None,
            basis_role: "source".into(),
        }));
    Ok(projection)
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

fn project_context(
    source_revision: CognitiveRef,
    mut text: String,
    basis: Vec<RevisionBasis>,
    max_bytes: usize,
) -> MemoryContextProjection {
    let evidence = basis
        .into_iter()
        .map(|basis| match basis {
            RevisionBasis::Evidence(value) => EvidenceHandle {
                reference: value.cognitive_ref(),
                epistemic_relation: value.epistemic_relation,
                basis_role: value.basis_role.as_str().into(),
            },
            RevisionBasis::CognitionDependency(value) => EvidenceHandle {
                reference: value.target_revision,
                epistemic_relation: value.epistemic_relation,
                basis_role: value.basis_role.as_str().into(),
            },
            RevisionBasis::Seed(value) => EvidenceHandle {
                reference: CognitiveRef::CognitiveSeedVersion(value.seed_version_id),
                epistemic_relation: None,
                basis_role: "seed".into(),
            },
        })
        .collect();
    let text = (max_bytes > 0).then(|| {
        let mut end = text.len().min(max_bytes);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text
    });
    MemoryContextProjection {
        source_revision,
        authority: AuthorityClass::SubjectCognition,
        text,
        evidence,
    }
}
