// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use std::collections::BTreeSet;

/// Authority-resolved provenance facts used by formation validation and
/// diagnostics.  The set is deliberately derived from immutable references;
/// it is not a mutable confidence or ranking field.
#[derive(Debug, Clone, Default)]
pub struct ProvenanceSummary {
    pub roots: BTreeSet<EvidenceRoot>,
    pub normalized_inputs: BTreeSet<String>,
}

impl MemoryService {
    pub async fn references_producer(&self, subject: SubjectId, producer: Uuid) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM memory_revisions WHERE subject_id=$1 AND producer_signature_id=$2
                UNION ALL SELECT 1 FROM episode_revisions WHERE subject_id=$1 AND producer_signature_id=$2
                UNION ALL SELECT 1 FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE o.subject_id=$1 AND r.producer_signature_id=$2
                UNION ALL SELECT 1 FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.producer_signature_id=$2
                UNION ALL SELECT 1 FROM tag_revisions r JOIN tags t USING(tag_id) WHERE t.subject_id=$1 AND r.producer_signature_id=$2
                UNION ALL SELECT 1 FROM association_evidence WHERE subject_id=$1 AND producer_signature_id=$2
            )",
        )
        .bind(subject.0)
        .bind(producer)
        .fetch_one(self.store.pool())
        .await
        .map_err(db)
    }

    pub(crate) async fn validate_basis_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        let occurrences = basis
            .iter()
            .filter_map(|basis| match basis {
                RevisionBasis::Evidence(e) => Some(e.occurrence_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        self.material
            .lock_occurrences_in(tx, subject, &occurrences)
            .await?;
        for basis in basis {
            match basis {
                RevisionBasis::Evidence(_) => {}
                RevisionBasis::CognitionDependency(dependency) => {
                    let (query, id) = dependency_query(&dependency.target_revision)?;
                    let row = sqlx::query(query)
                        .bind(subject.0)
                        .bind(id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(db)?;
                    row.ok_or_else(|| {
                        Error::Invalid("cognition dependency is outside Subject".into())
                    })?;
                }
                RevisionBasis::Seed(_) => {
                    return Err(Error::Invalid(
                        "Memory revisions cannot use Cognitive Seed support".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn validate_object_dependency_cycle(
        &self,
        subject: SubjectId,
        owner: &CognitiveRef,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        let mut stack = basis
            .iter()
            .filter_map(|basis| match basis {
                RevisionBasis::CognitionDependency(value) => Some(value.target_revision.clone()),
                RevisionBasis::Evidence(_) => None,
                RevisionBasis::Seed(_) => None,
            })
            .collect::<Vec<_>>();
        if stack.is_empty() {
            return Ok(());
        }
        let mut visited = std::collections::HashSet::new();
        let mut tx = self.store.read_view().await?;
        while !stack.is_empty() {
            let references = stack
                .drain(..stack.len().min(256))
                .filter(|reference| visited.insert(reference.clone()))
                .collect::<Vec<_>>();
            for support in self
                .exact_supports_in(&mut tx, subject, &references, None)
                .await?
                .into_values()
            {
                if &support.object == owner {
                    return Err(Error::Invalid(
                        "cognition dependency would create an object cycle".into(),
                    ));
                }
                stack.extend(support.basis.into_iter().filter_map(|basis| match basis {
                    RevisionBasis::CognitionDependency(value) => Some(value.target_revision),
                    _ => None,
                }));
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    pub async fn provenance_summary(
        &self,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<ProvenanceSummary> {
        self.provenance_summary_in_view(subject, basis, None).await
    }

    pub async fn provenance_summary_in_view(
        &self,
        subject: SubjectId,
        basis: &[RevisionBasis],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<ProvenanceSummary> {
        if view.is_some_and(|view| view.subject != subject) {
            return Err(Error::Invalid("provenance view subject mismatch".into()));
        }
        let mut summary = ProvenanceSummary::default();
        let mut stack = Vec::new();
        let mut evidence_inputs = Vec::new();
        for basis in basis {
            match basis {
                RevisionBasis::Evidence(evidence) => {
                    evidence_inputs.push(evidence.clone());
                }
                RevisionBasis::CognitionDependency(dependency) => {
                    let key = dependency.target_revision.to_string();
                    summary.normalized_inputs.insert(format!("revision:{key}"));
                    stack.push((dependency.target_revision.clone(), false));
                }
                RevisionBasis::Seed(_) => {
                    return Err(Error::Invalid(
                        "Memory revisions cannot use Cognitive Seed support".into(),
                    ));
                }
            }
        }

        let direct_evidence = evidence_inputs.len();
        let mut active = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        let mut supports = std::collections::HashMap::new();
        let mut tx = if stack.is_empty() {
            None
        } else {
            Some(self.store.read_view().await?)
        };
        while let Some((reference, exit)) = stack.pop() {
            if exit {
                active.remove(&reference);
                visited.insert(reference);
                continue;
            }
            if active.contains(&reference) {
                return Err(Error::FailedPrecondition(
                    "cognition dependency cycle detected".into(),
                ));
            }
            if visited.contains(&reference) {
                continue;
            }
            active.insert(reference.clone());
            stack.push((reference.clone(), true));
            if !supports.contains_key(&reference) {
                let references = std::iter::once(reference.clone())
                    .chain(
                        stack
                            .iter()
                            .filter(|(r, exit)| {
                                !exit && !visited.contains(r) && !active.contains(r)
                            })
                            .map(|(r, _)| r.clone()),
                    )
                    .filter(|r| !supports.contains_key(r))
                    .take(256)
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                supports.extend(
                    self.exact_supports_in(
                        tx.as_mut().expect("dependency read"),
                        subject,
                        &references,
                        view,
                    )
                    .await?,
                );
            }
            let nested = supports
                .remove(&reference)
                .expect("loaded cognition support")
                .basis;
            for basis in nested.into_iter().rev() {
                match basis {
                    RevisionBasis::Evidence(evidence) => {
                        evidence_inputs.push(evidence);
                    }
                    RevisionBasis::CognitionDependency(dependency) => {
                        stack.push((dependency.target_revision, false));
                    }
                    RevisionBasis::Seed(_) => {
                        return Err(Error::Invalid(
                            "Memory revisions cannot use Cognitive Seed support".into(),
                        ));
                    }
                }
            }
        }
        if let Some(tx) = tx {
            tx.commit().await.map_err(db)?;
        }
        for (batch_index, evidence) in evidence_inputs.chunks(256).enumerate() {
            for (index, roots) in self
                .material
                .evidence_roots_batch(subject, evidence, view)
                .await?
                .into_iter()
                .enumerate()
            {
                if batch_index * 256 + index < direct_evidence {
                    summary
                        .normalized_inputs
                        .extend(roots.iter().map(|root| format!("source:{}", root.root_key)));
                }
                summary.roots.extend(roots);
            }
        }
        Ok(summary)
    }

    pub async fn provenance_for_reference(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeSet<EvidenceRoot>> {
        self.store.validate_reference(subject, reference).await?;
        let basis = match reference {
            CognitiveRef::MemoryRevision(_)
            | CognitiveRef::EpisodeRevision(_)
            | CognitiveRef::JournalRevision(_)
            | CognitiveRef::CognitiveSchemaRevision(_) => {
                self.exact_supports(subject, std::slice::from_ref(reference), view)
                    .await?
                    .remove(reference)
                    .expect("loaded cognition support")
                    .basis
            }
            CognitiveRef::Association(id) => {
                if view
                    .is_some_and(|view| view.subject != subject || !view.association_contains(*id))
                {
                    return Err(Error::NotFound(
                        "association is outside captured view".into(),
                    ));
                }
                self.association(subject, *id)
                    .await?
                    .basis
                    .into_iter()
                    .filter_map(|basis| match basis {
                        AssociationBasis::Revision(basis) => Some(basis),
                        // Meaningful Use supports an association, not an independent factual source.
                        AssociationBasis::UseEvent(_) => None,
                    })
                    .collect()
            }
            CognitiveRef::Tag(_) => return Ok(BTreeSet::new()),
            _ => {
                return Err(Error::Invalid(
                    "provenance requires an owned exact cognition/association".into(),
                ));
            }
        };
        Ok(self
            .provenance_summary_in_view(subject, &basis, view)
            .await?
            .roots)
    }

    pub(in crate::service) fn validate_synthesized_summary(
        summary: &ProvenanceSummary,
    ) -> Result<()> {
        if summary.normalized_inputs.len() < 2 {
            return Err(Error::Invalid(
                "synthesized formation needs at least two normalized inputs".into(),
            ));
        }
        let independent_roots = summary
            .roots
            .iter()
            .filter(|root| matches!(root.certainty, EvidenceRootCertainty::Known))
            .count();
        if independent_roots < 2 {
            return Err(Error::Invalid(
                "synthesized formation needs two known independent provenance roots".into(),
            ));
        }
        Ok(())
    }
}

fn dependency_query(reference: &CognitiveRef) -> Result<(&'static str, Uuid)> {
    Ok(match reference {
        CognitiveRef::MemoryRevision(id) => (
            "SELECT r.memory_id FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::CognitiveSchemaRevision(id) => (
            "SELECT r.schema_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::EpisodeRevision(id) => (
            "SELECT r.episode_id FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        CognitiveRef::JournalRevision(id) => (
            "SELECT r.journal_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2 FOR SHARE OF o",
            id.0,
        ),
        _ => {
            return Err(Error::Invalid(
                "cognition dependency needs exact revision".into(),
            ));
        }
    })
}
