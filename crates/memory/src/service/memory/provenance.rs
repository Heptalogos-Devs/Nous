// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub(in crate::service) async fn validate_basis_for_subject(
        &self,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<()> {
        for basis in basis {
            match basis {
                RevisionBasis::Evidence(evidence) => {
                    self.validate_evidence(subject, evidence).await?
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
                            nous_core::ErrorOwner::Memory,
                            nous_core::ErrorCode::InvalidRequest,
                            "cognition dependency must target an exact revision",
                        ));
                    }
                    self.store
                        .validate_reference(subject, &dependency.target_revision)
                        .await?;
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
        // Resolve the complete exact-revision dependency closure before the
        // mutation starts.  This is also the deterministic cycle check; a
        // model or caller cannot bypass it by presenting a syntactically
        // valid direct edge.
        self.provenance_summary(subject, basis).await?;
        Ok(())
    }

    pub(in crate::service) async fn validate_evidence(
        &self,
        subject: SubjectId,
        evidence: &EvidenceRef,
    ) -> Result<()> {
        let row = sqlx::query("SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2")
            .bind(subject.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(|| Error::new(nous_core::ErrorOwner::Memory, nous_core::ErrorCode::InvalidRequest, "evidence occurrence is outside Subject"))?;
        let occurrence_artifact: Option<Uuid> = row.try_get("artifact_id").map_err(db)?;
        match evidence.locator {
            EvidenceLocator::WholeOccurrence => {}
            EvidenceLocator::SourceRegion(id) => {
                let artifact: Uuid = sqlx::query_scalar("SELECT artifact_id FROM source_regions WHERE subject_id=$1 AND source_region_id=$2").bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
                if Some(artifact) != occurrence_artifact {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "source region artifact does not match occurrence",
                    ));
                }
            }
            EvidenceLocator::DerivedRepresentation(id) => {
                let artifact: Option<Uuid> = sqlx::query_scalar("SELECT sr.artifact_id FROM representation_source_regions($1,$2) roots JOIN source_regions sr USING(source_region_id) WHERE sr.artifact_id=(SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$3)").bind(subject.0).bind(id.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.flatten();
                if artifact.is_none() || artifact != occurrence_artifact {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "derived representation source does not match occurrence",
                    ));
                }
            }
            EvidenceLocator::DerivedRegion(id) => {
                let artifact: Option<Uuid> = sqlx::query_scalar("SELECT sr.artifact_id FROM derived_regions dr JOIN LATERAL representation_source_regions($1,dr.derived_representation_id) roots ON true JOIN source_regions sr USING(source_region_id) WHERE dr.subject_id=$1 AND dr.derived_region_id=$2 AND sr.artifact_id=(SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$3)").bind(subject.0).bind(id.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.flatten();
                if artifact.is_none() || artifact != occurrence_artifact {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "derived region source does not match occurrence",
                    ));
                }
            }
        }
        Ok(())
    }
}
