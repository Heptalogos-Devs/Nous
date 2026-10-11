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
    /// Context projection is an ordinary current read, including explicit refs.
    /// A missing or hidden owned cognition must never become external authority.
    pub async fn contextual_cognition_eligible(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<bool> {
        let exact = match self.store.bind_exact_reference(subject, reference).await {
            Ok((exact, _, _)) => exact,
            Err(Error::NotFound(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        let (query, id) = match exact {
            CognitiveRef::MemoryRevision(id) => (
                "SELECT EXISTS(SELECT 1 FROM memory_objects o JOIN memory_revisions r USING(memory_id) WHERE o.subject_id=$1 AND r.memory_revision_id=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal')",
                id.0,
            ),
            CognitiveRef::CognitiveSchemaRevision(id) => (
                "SELECT EXISTS(SELECT 1 FROM cognitive_schemas o JOIN cognitive_schema_revisions r USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal')",
                id.0,
            ),
            CognitiveRef::EpisodeRevision(id) => (
                "SELECT EXISTS(SELECT 1 FROM episode_objects o JOIN episode_revisions r USING(episode_id) WHERE o.subject_id=$1 AND r.episode_revision_id=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal')",
                id.0,
            ),
            CognitiveRef::JournalRevision(id) => (
                "SELECT EXISTS(SELECT 1 FROM journal_objects o JOIN journal_revisions r USING(journal_id) WHERE o.subject_id=$1 AND r.journal_revision_id=$2 AND o.acceptance_state='accepted' AND o.integrity_state='valid' AND o.suppression_state='normal' AND o.purge_state='normal')",
                id.0,
            ),
            _ => {
                return Err(Error::Invalid(
                    "contextual cognition requires an owned cognition reference".into(),
                ));
            }
        };
        sqlx::query_scalar(query)
            .bind(subject.0)
            .bind(id)
            .fetch_one(self.store.pool())
            .await
            .map_err(db)
    }

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
        for basis in basis {
            match basis {
                RevisionBasis::Evidence(evidence) => {
                    sqlx::query("SELECT occurrence_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2 FOR SHARE")
                        .bind(subject.0).bind(evidence.occurrence_id.0)
                        .fetch_optional(&mut **tx).await.map_err(db)?
                        .ok_or_else(|| Error::Invalid("evidence occurrence is outside Subject".into()))?;
                }
                RevisionBasis::CognitionDependency(dependency) => {
                    let (query, id, _) = dependency_query(&dependency.target_revision, true)?;
                    let row = sqlx::query(query)
                        .bind(subject.0)
                        .bind(id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(db)?;
                    require_dependency_row(row)?;
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
        owner_key: &str,
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
        let mut visited = BTreeSet::new();
        while let Some(reference) = stack.pop() {
            let object_key = self.revision_object_key(subject, &reference).await?;
            if object_key == owner_key {
                return Err(Error::Invalid(
                    "cognition dependency would create an object cycle".into(),
                ));
            }
            if !visited.insert(reference.to_string()) {
                continue;
            }
            for nested in self.revision_basis(subject, reference).await? {
                if let RevisionBasis::CognitionDependency(value) = nested {
                    stack.push(value.target_revision);
                }
            }
        }
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
        for basis in basis {
            match basis {
                RevisionBasis::Evidence(evidence) => {
                    for root in self
                        .material
                        .evidence_roots(subject, evidence, view)
                        .await?
                    {
                        summary
                            .normalized_inputs
                            .insert(format!("source:{}", root.root_key));
                        summary.roots.insert(root);
                    }
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

        let mut active = BTreeSet::new();
        let mut visited = BTreeSet::new();
        while let Some((reference, exit)) = stack.pop() {
            let key = reference.to_string();
            if exit {
                active.remove(&key);
                visited.insert(key);
                continue;
            }
            if active.contains(&key) {
                return Err(Error::FailedPrecondition(
                    "cognition dependency cycle detected".into(),
                ));
            }
            if visited.contains(&key) {
                continue;
            }
            self.validate_dependency_target(subject, &reference).await?;
            active.insert(key.clone());
            stack.push((reference.clone(), true));
            let nested = self
                .revision_basis_in_view(subject, reference, view)
                .await?;
            for basis in nested.into_iter().rev() {
                match basis {
                    RevisionBasis::Evidence(evidence) => {
                        summary.roots.extend(
                            self.material
                                .evidence_roots(subject, &evidence, view)
                                .await?,
                        );
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
                self.revision_basis_in_view(subject, reference.clone(), view)
                    .await?
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

    pub(crate) async fn validate_formation_semantics(
        &self,
        subject: SubjectId,
        mode: FormationMode,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        if !matches!(mode, FormationMode::Synthesized) {
            return Ok(());
        }
        let summary = self.provenance_summary(subject, basis).await?;
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

    async fn validate_dependency_target(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        let (query, id, _) = dependency_query(reference, false)?;
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?;
        require_dependency_row(row)
    }

    async fn revision_object_key(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<String> {
        let (query, id, kind) = dependency_query(reference, false)?;
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::Invalid("cognition dependency is outside Subject".into()))?;
        let object: Uuid = row.try_get("object_id").map_err(db)?;
        Ok(format!("{kind}:{object}"))
    }

    async fn revision_basis(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
    ) -> Result<Vec<RevisionBasis>> {
        self.revision_basis_in_view(subject, reference, None).await
    }

    async fn revision_basis_in_view(
        &self,
        subject: SubjectId,
        reference: CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<RevisionBasis>> {
        if view.is_some_and(|view| view.cognition_for(&reference).is_none()) {
            return Err(Error::NotFound(
                "cognition support is outside the captured view".into(),
            ));
        }
        match reference {
            CognitiveRef::MemoryRevision(revision) => self.load_basis(revision).await,
            CognitiveRef::CognitiveSchemaRevision(revision) => Ok(self
                .schema_links_in_view(subject, revision, view)
                .await?
                .into_iter()
                .map(|link| link.basis)
                .collect()),
            CognitiveRef::EpisodeRevision(revision) => {
                Ok(self.episode_revision(subject, revision).await?.basis)
            }
            CognitiveRef::JournalRevision(revision) => Ok(self
                .journal_revision(subject, revision)
                .await?
                .points
                .into_iter()
                .flat_map(|point| point.basis)
                .collect()),
            _ => Err(Error::Invalid(
                "cognition dependency needs exact revision".into(),
            )),
        }
    }
}

fn dependency_query(
    reference: &CognitiveRef,
    lock: bool,
) -> Result<(&'static str, Uuid, &'static str)> {
    match reference {
        CognitiveRef::MemoryRevision(id) => Ok((
            if lock {
                "SELECT o.purge_state,r.memory_id AS object_id FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT o.purge_state,r.memory_id AS object_id FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2"
            },
            id.0,
            "memory",
        )),
        CognitiveRef::CognitiveSchemaRevision(id) => Ok((
            if lock {
                "SELECT o.purge_state,r.schema_id AS object_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT o.purge_state,r.schema_id AS object_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2"
            },
            id.0,
            "schema",
        )),
        CognitiveRef::EpisodeRevision(id) => Ok((
            if lock {
                "SELECT o.purge_state,r.episode_id AS object_id FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT o.purge_state,r.episode_id AS object_id FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2"
            },
            id.0,
            "episode",
        )),
        CognitiveRef::JournalRevision(id) => Ok((
            if lock {
                "SELECT o.purge_state,r.journal_id AS object_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT o.purge_state,r.journal_id AS object_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2"
            },
            id.0,
            "journal",
        )),
        _ => Err(Error::Invalid(
            "cognition dependency needs exact revision".into(),
        )),
    }
}

fn require_dependency_row(row: Option<sqlx::postgres::PgRow>) -> Result<()> {
    let state = row
        .map(|row| row.try_get::<String, _>("purge_state").map_err(db))
        .transpose()?;
    match state.as_deref() {
        None => Err(Error::Invalid(
            "cognition dependency is outside Subject".into(),
        )),
        Some("purging") => Err(Error::FailedPrecondition(
            "cognition dependency target is purging".into(),
        )),
        Some(_) => Ok(()),
    }
}
