// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

impl MemoryService {
    pub(in crate::service) async fn validate_basis_for_subject(
        &self,
        subject: SubjectId,
        basis: &[RevisionBasis],
    ) -> Result<super::super::provenance::ProvenanceSummary> {
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
                        return Err(Error::Invalid(
                            "cognition dependency must target an exact revision".into(),
                        ));
                    }
                    self.store
                        .validate_reference(subject, &dependency.target_revision)
                        .await?;
                }
                RevisionBasis::Seed(_) => {
                    return Err(Error::Invalid(
                        "Memory revisions cannot use Cognitive Seed support".into(),
                    ));
                }
            }
        }
        // Resolve the complete exact-revision dependency closure before the
        // write. The returned summary also supplies synthesized-formation
        // roots, so the closure is not traversed a second time.
        self.provenance_summary(subject, basis).await
    }

    pub(in crate::service) async fn validate_evidence(
        &self,
        subject: SubjectId,
        evidence: &EvidenceRef,
    ) -> Result<()> {
        let row = sqlx::query("SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2")
            .bind(subject.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.ok_or_else(|| Error::Invalid("evidence occurrence is outside Subject".into()))?;
        let occurrence_artifact: Option<Uuid> = row.try_get("artifact_id").map_err(db)?;
        match evidence.locator {
            EvidenceLocator::WholeOccurrence => {}
            EvidenceLocator::SourceRegion(id) => {
                let artifact: Uuid = sqlx::query_scalar("SELECT artifact_id FROM source_regions WHERE subject_id=$1 AND source_region_id=$2").bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
                if Some(artifact) != occurrence_artifact {
                    return Err(Error::Invalid(
                        "source region artifact does not match occurrence".into(),
                    ));
                }
            }
            EvidenceLocator::DerivedRepresentation(id) => {
                let artifact: Option<Uuid> = sqlx::query_scalar("SELECT sr.artifact_id FROM representation_source_regions($1,$2) roots JOIN source_regions sr USING(source_region_id) WHERE sr.artifact_id=(SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$3)").bind(subject.0).bind(id.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.flatten();
                if artifact.is_none() || artifact != occurrence_artifact {
                    return Err(Error::Invalid(
                        "derived representation source does not match occurrence".into(),
                    ));
                }
            }
            EvidenceLocator::DerivedRegion(id) => {
                let artifact: Option<Uuid> = sqlx::query_scalar("SELECT sr.artifact_id FROM derived_regions dr JOIN LATERAL representation_source_regions($1,dr.derived_representation_id) roots ON true JOIN source_regions sr USING(source_region_id) WHERE dr.subject_id=$1 AND dr.derived_region_id=$2 AND sr.artifact_id=(SELECT artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$3)").bind(subject.0).bind(id.0).bind(evidence.occurrence_id.0).fetch_optional(self.store.pool()).await.map_err(db)?.flatten();
                if artifact.is_none() || artifact != occurrence_artifact {
                    return Err(Error::Invalid(
                        "derived region source does not match occurrence".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}
