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

pub(in crate::service) struct CognitionSupport {
    pub object: CognitiveRef,
    pub basis: Vec<RevisionBasis>,
    pub members: Vec<EpisodeMember>,
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
        let mut visited = BTreeSet::new();
        while let Some(reference) = stack.pop() {
            let support = self.exact_support(subject, &reference, None).await?;
            if &support.object == owner {
                return Err(Error::Invalid(
                    "cognition dependency would create an object cycle".into(),
                ));
            }
            if !visited.insert(reference.to_string()) {
                continue;
            }
            for nested in support.basis {
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
            active.insert(key.clone());
            stack.push((reference.clone(), true));
            let nested = self.exact_support(subject, &reference, view).await?.basis;
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
                self.exact_support(subject, reference, view).await?.basis
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

    pub(in crate::service) async fn exact_support(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<CognitionSupport> {
        if view
            .is_some_and(|view| view.subject != subject || view.cognition_for(reference).is_none())
        {
            return Err(Error::NotFound(
                "cognition support is outside the captured view".into(),
            ));
        }
        let (query, id, kind) = dependency_query(reference, false)?;
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::Invalid("cognition dependency is outside Subject".into()))?;
        let object: Uuid = row.try_get("object_id").map_err(db)?;
        let object = match kind {
            "memory" => CognitiveRef::Memory(MemoryId(object)),
            "schema" => CognitiveRef::CognitiveSchema(CognitiveSchemaId(object)),
            "episode" => CognitiveRef::Episode(EpisodeId(object)),
            "journal" => CognitiveRef::Journal(JournalId(object)),
            _ => unreachable!("dependency kind is validated"),
        };
        let mut members = Vec::new();
        let basis = match reference {
            CognitiveRef::MemoryRevision(revision) => self.load_basis(*revision).await?,
            CognitiveRef::CognitiveSchemaRevision(revision) => self.schema_links_in_view(subject, *revision, view).await?.into_iter().map(|link| link.basis).collect(),
            CognitiveRef::EpisodeRevision(revision) => {
                let support = self.episode_support(revision.0).await?;
                members = support.1;
                support.0
            }
            CognitiveRef::JournalRevision(revision) => sqlx::query_scalar::<_,serde_json::Value>("SELECT basis FROM journal_point_basis WHERE journal_revision_id=$1 ORDER BY ordinal,basis_no")
                .bind(revision.0).fetch_all(self.store.pool()).await.map_err(db)?.into_iter()
                .map(|value| serde_json::from_value(value).map_err(|error| Error::Infrastructure(error.to_string()))).collect::<Result<_>>()?,
            _ => unreachable!("dependency requires an exact revision"),
        };
        Ok(CognitionSupport {
            object,
            basis,
            members,
        })
    }
}

fn dependency_query(
    reference: &CognitiveRef,
    lock: bool,
) -> Result<(&'static str, Uuid, &'static str)> {
    match reference {
        CognitiveRef::MemoryRevision(id) => Ok((
            if lock {
                "SELECT r.memory_id AS object_id FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT r.memory_id AS object_id FROM memory_revisions r JOIN memory_objects o USING(memory_id) WHERE r.subject_id=$1 AND r.memory_revision_id=$2"
            },
            id.0,
            "memory",
        )),
        CognitiveRef::CognitiveSchemaRevision(id) => Ok((
            if lock {
                "SELECT r.schema_id AS object_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT r.schema_id AS object_id FROM cognitive_schema_revisions r JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1 AND r.schema_revision_id=$2"
            },
            id.0,
            "schema",
        )),
        CognitiveRef::EpisodeRevision(id) => Ok((
            if lock {
                "SELECT r.episode_id AS object_id FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT r.episode_id AS object_id FROM episode_revisions r JOIN episode_objects o USING(episode_id) WHERE r.subject_id=$1 AND r.episode_revision_id=$2"
            },
            id.0,
            "episode",
        )),
        CognitiveRef::JournalRevision(id) => Ok((
            if lock {
                "SELECT r.journal_id AS object_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2 FOR SHARE OF o"
            } else {
                "SELECT r.journal_id AS object_id FROM journal_revisions r JOIN journal_objects o USING(journal_id) WHERE r.subject_id=$1 AND r.journal_revision_id=$2"
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
    row.map(|_| ())
        .ok_or_else(|| Error::Invalid("cognition dependency is outside Subject".into()))
}
