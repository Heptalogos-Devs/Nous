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
                SELECT 1 FROM cognition_revision_headers WHERE subject_id=$1 AND producer_signature_id=$2
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
        owner: ErrorOwner,
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
                        .ok_or_else(|| Error::new(owner, nous_core::ErrorCode::InvalidRequest, "evidence occurrence is outside Subject"))?;
                }
                RevisionBasis::CognitionDependency(dependency) => {
                    if !matches!(
                        dependency.target_revision,
                        CognitiveRef::MemoryRevision(_)
                            | CognitiveRef::CognitiveSchemaRevision(_)
                            | CognitiveRef::EpisodeRevision(_)
                            | CognitiveRef::JournalRevision(_)
                    ) {
                        return Err(Error::new(
                            owner,
                            nous_core::ErrorCode::InvalidRequest,
                            "cognition dependency must target an exact revision",
                        ));
                    }
                    let (query, id, kind) = dependency_query(&dependency.target_revision, true)?;
                    let row = sqlx::query(query)
                        .bind(subject.0)
                        .bind(kind.as_str())
                        .bind(id)
                        .fetch_optional(&mut **tx)
                        .await
                        .map_err(db)?;
                    require_dependency_row(owner, row)?;
                }
                RevisionBasis::Seed(_) => {
                    let message = match owner {
                        ErrorOwner::Memory => "Memory revisions cannot use Cognitive Seed support",
                        ErrorOwner::Concept => {
                            "Concept revisions cannot use Cognitive Seed support"
                        }
                        _ => "Cognitive revisions cannot use Cognitive Seed support",
                    };
                    return Err(Error::new(
                        owner,
                        nous_core::ErrorCode::InvalidRequest,
                        message,
                    ));
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn validate_object_dependency_cycle(
        &self,
        owner: ErrorOwner,
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
                return Err(Error::new(
                    owner,
                    nous_core::ErrorCode::InvalidRequest,
                    "cognition dependency would create an object cycle",
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "provenance view subject mismatch",
            ));
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
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Memory revisions cannot use Cognitive Seed support",
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
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::FailedPrecondition,
                    "cognition dependency cycle detected",
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
                        return Err(Error::new(
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InvalidRequest,
                            "Memory revisions cannot use Cognitive Seed support",
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
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::NotFound,
                        "association is outside captured view",
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
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "provenance requires an owned exact cognition/association",
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
        owner: ErrorOwner,
        subject: SubjectId,
        mode: FormationMode,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        if !matches!(mode, FormationMode::Synthesized) {
            return Ok(());
        }
        let summary = self.provenance_summary(subject, basis).await?;
        if summary.normalized_inputs.len() < 2 {
            return Err(Error::new(
                owner,
                nous_core::ErrorCode::InvalidRequest,
                "synthesized formation needs at least two normalized inputs",
            ));
        }
        let independent_roots = summary
            .roots
            .iter()
            .filter(|root| matches!(root.certainty, EvidenceRootCertainty::Known))
            .count();
        if independent_roots < 2 {
            return Err(Error::new(
                owner,
                nous_core::ErrorCode::InvalidRequest,
                "synthesized formation needs two known independent provenance roots",
            ));
        }
        Ok(())
    }

    async fn validate_dependency_target(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<()> {
        let (query, id, kind) = dependency_query(reference, false)?;
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?;
        require_dependency_row(ErrorOwner::Memory, row)
    }

    async fn revision_object_key(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
    ) -> Result<String> {
        let (query, id, kind) = dependency_query(reference, false)?;
        let row = sqlx::query(query)
            .bind(subject.0)
            .bind(kind.as_str())
            .bind(id)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InvalidRequest,
                    "cognition dependency is outside Subject",
                )
            })?;
        let object: Uuid = row.try_get("object_id").map_err(db)?;
        let family = if kind == CognitionKind::CognitiveSchema {
            "schema"
        } else {
            kind.as_str()
        };
        Ok(format!("{family}:{object}"))
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
            return Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::NotFound,
                "cognition support is outside the captured view",
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
            _ => Err(Error::new(
                nous_core::ErrorOwner::Memory,
                nous_core::ErrorCode::InvalidRequest,
                "cognition dependency needs exact revision",
            )),
        }
    }
}

fn dependency_query(
    reference: &CognitiveRef,
    lock: bool,
) -> Result<(&'static str, Uuid, CognitionKind)> {
    let (kind, id, is_revision) = CognitionKind::from_reference(reference)?;
    if !is_revision {
        return Err(Error::new(
            ErrorOwner::Memory,
            ErrorCode::InvalidRequest,
            "cognition dependency needs exact revision",
        ));
    }
    let query = if lock {
        "SELECT o.purge_state,o.object_id FROM cognition_revision_headers h JOIN cognition_objects o ON o.subject_id=h.subject_id AND o.object_kind=h.object_kind AND o.object_id=h.object_id WHERE h.subject_id=$1 AND h.object_kind=$2 AND h.revision_id=$3 FOR SHARE OF o"
    } else {
        "SELECT o.purge_state,o.object_id FROM cognition_revision_headers h JOIN cognition_objects o ON o.subject_id=h.subject_id AND o.object_kind=h.object_kind AND o.object_id=h.object_id WHERE h.subject_id=$1 AND h.object_kind=$2 AND h.revision_id=$3"
    };
    Ok((query, id, kind))
}

fn require_dependency_row(owner: ErrorOwner, row: Option<sqlx::postgres::PgRow>) -> Result<()> {
    let state = row
        .map(|row| row.try_get::<String, _>("purge_state").map_err(db))
        .transpose()?;
    match state.as_deref() {
        None => Err(Error::new(
            owner,
            nous_core::ErrorCode::InvalidRequest,
            "cognition dependency is outside Subject",
        )),
        Some("purging") => Err(Error::new(
            owner,
            nous_core::ErrorCode::FailedPrecondition,
            "cognition dependency target is purging",
        )),
        Some(_) => Ok(()),
    }
}
