// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_persistence::database_error as db;
use std::collections::BTreeSet;

impl MaterialService {
    /// Material owns source independence; cognition owners combine these roots.
    pub async fn evidence_roots(
        &self,
        subject: SubjectId,
        evidence: &EvidenceRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeSet<EvidenceRoot>> {
        self.require_lineage_view(
            subject,
            &CognitiveRef::Occurrence(evidence.occurrence_id),
            view,
        )?;
        self.store
            .validate_reference(subject, &CognitiveRef::Occurrence(evidence.occurrence_id))
            .await?;
        match evidence.locator {
            EvidenceLocator::SourceRegion(id) => {
                self.lineage_roots(subject, &CognitiveRef::SourceRegion(id), view)
                    .await
            }
            EvidenceLocator::DerivedRepresentation(id) => {
                self.lineage_roots(subject, &CognitiveRef::DerivedRepresentation(id), view)
                    .await
            }
            EvidenceLocator::DerivedRegion(id) => {
                self.lineage_roots(subject, &CognitiveRef::DerivedRegion(id), view)
                    .await
            }
            _ => Ok(BTreeSet::from([self
                .occurrence_root(subject, evidence.occurrence_id, view)
                .await?])),
        }
    }

    pub async fn lineage_roots(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeSet<EvidenceRoot>> {
        self.require_lineage_view(subject, reference, view)?;
        self.store.validate_reference(subject, reference).await?;
        match reference {
            CognitiveRef::Occurrence(id) => Ok(BTreeSet::from([self
                .occurrence_root(subject, *id, view)
                .await?])),
            CognitiveRef::Artifact(id) => Ok(BTreeSet::from([self
                .artifact_root(subject, id.0, view)
                .await?])),
            CognitiveRef::SourceRegion(id) => {
                let artifact: Uuid = sqlx::query_scalar("SELECT artifact_id FROM source_regions WHERE subject_id=$1 AND source_region_id=$2")
                    .bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
                Ok(BTreeSet::from([self
                    .artifact_root(subject, artifact, view)
                    .await?]))
            }
            CognitiveRef::DerivedRepresentation(id) => {
                self.representation_roots(subject, id.0, view).await
            }
            CognitiveRef::DerivedRegion(id) => {
                let representation: Uuid = sqlx::query_scalar("SELECT derived_representation_id FROM derived_regions WHERE subject_id=$1 AND derived_region_id=$2")
                    .bind(subject.0).bind(id.0).fetch_one(self.store.pool()).await.map_err(db)?;
                self.representation_roots(subject, representation, view)
                    .await
            }
            _ => Err(Error::Invalid(
                "lineage requires a Material reference".into(),
            )),
        }
    }

    pub(super) fn require_lineage_view(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<()> {
        if view.is_some_and(|view| view.subject != subject || !view.material_contains(reference)) {
            return Err(Error::NotFound(
                "Material reference is outside the captured view".into(),
            ));
        }
        Ok(())
    }

    async fn occurrence_root(
        &self,
        subject: SubjectId,
        occurrence: OccurrenceId,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<EvidenceRoot> {
        let row = sqlx::query("SELECT source_class,external_object_ref,artifact_id FROM observation_occurrences WHERE subject_id=$1 AND occurrence_id=$2 AND ($3::timestamptz IS NULL OR created_at<=$3)")
            .bind(subject.0).bind(occurrence.0).bind(view.map(|v|v.as_of)).fetch_optional(self.store.pool()).await.map_err(db)?
            .ok_or_else(|| Error::Invalid("evidence occurrence is outside Subject/view".into()))?;
        if let Some(artifact) = row.try_get::<Option<Uuid>, _>("artifact_id").map_err(db)? {
            return self.artifact_root(subject, artifact, view).await;
        }
        if let Some(external) = row
            .try_get::<Option<String>, _>("external_object_ref")
            .map_err(db)?
        {
            let class: String = row.try_get("source_class").map_err(db)?;
            return Ok(EvidenceRoot {
                root_key: format!("external:{class}:{external}"),
                certainty: EvidenceRootCertainty::Known,
            });
        }
        Ok(EvidenceRoot {
            root_key: format!("occurrence:{}", occurrence.0),
            certainty: EvidenceRootCertainty::OccurrenceOnly,
        })
    }

    async fn representation_roots(
        &self,
        subject: SubjectId,
        representation: Uuid,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeSet<EvidenceRoot>> {
        let artifacts: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT sr.artifact_id FROM representation_source_regions($1,$2) roots JOIN source_regions sr USING(source_region_id) WHERE sr.subject_id=$1 AND ($3::timestamptz IS NULL OR sr.created_at<=$3)")
            .bind(subject.0).bind(representation).bind(view.map(|v|v.as_of)).fetch_all(self.store.pool()).await.map_err(db)?;
        if artifacts.is_empty() {
            return Err(Error::Invalid(
                "derived evidence has no valid source roots".into(),
            ));
        }
        let mut roots = BTreeSet::new();
        for artifact in artifacts {
            roots.insert(self.artifact_root(subject, artifact, view).await?);
        }
        Ok(roots)
    }

    async fn artifact_root(
        &self,
        subject: SubjectId,
        artifact: Uuid,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<EvidenceRoot> {
        let external: Option<String> = sqlx::query_scalar("WITH RECURSIVE lineage AS (SELECT occurrence_id,artifact_id,source_class,external_object_ref FROM observation_occurrences WHERE subject_id=$1 AND artifact_id=$2 AND ($3::timestamptz IS NULL OR created_at<=$3) UNION SELECT o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref FROM observation_occurrences o JOIN lineage p ON (o.artifact_id IS NOT NULL AND o.artifact_id=p.artifact_id) OR (o.external_object_ref IS NOT NULL AND o.external_object_ref=p.external_object_ref AND o.source_class=p.source_class) WHERE o.subject_id=$1 AND ($3::timestamptz IS NULL OR o.created_at<=$3)) SELECT MIN('external:' || source_class || ':' || external_object_ref) FROM lineage WHERE external_object_ref IS NOT NULL")
            .bind(subject.0).bind(artifact).bind(view.map(|v|v.as_of)).fetch_one(self.store.pool()).await.map_err(db)?;
        Ok(EvidenceRoot {
            root_key: external.unwrap_or_else(|| format!("artifact:{artifact}")),
            certainty: EvidenceRootCertainty::Known,
        })
    }
}
