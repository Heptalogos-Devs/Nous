//! Material owns exact locator qualification and source independence in bounded batches.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::database_error as db;
use std::collections::{BTreeSet, HashMap, HashSet};

pub(super) fn evidence_reference(evidence: &EvidenceRef) -> CognitiveRef {
    match evidence.locator {
        EvidenceLocator::WholeOccurrence => CognitiveRef::Occurrence(evidence.occurrence_id),
        EvidenceLocator::SourceRegion(id) => CognitiveRef::SourceRegion(id),
        EvidenceLocator::DerivedRepresentation(id) => CognitiveRef::DerivedRepresentation(id),
        EvidenceLocator::DerivedRegion(id) => CognitiveRef::DerivedRegion(id),
    }
}

struct Lineage {
    roots: BTreeSet<EvidenceRoot>,
    artifacts: Vec<Uuid>,
}

const LINEAGE_READ: &str = r#"
WITH RECURSIVE requested AS (SELECT kind,id,ordinality FROM unnest($2::text[],$3::uuid[]) WITH ORDINALITY AS r(kind,id,ordinality)),
resolved AS (
 SELECT r.*,o.source_class,o.external_object_ref,
 o.occurrence_id IS NOT NULL OR a.artifact_id IS NOT NULL OR sr.source_region_id IS NOT NULL OR d.derived_representation_id IS NOT NULL OR dr.derived_region_id IS NOT NULL AS present,
 coalesce(o.artifact_id,a.artifact_id,sr.artifact_id) AS artifact_id,coalesce(d.derived_representation_id,dr.derived_representation_id) AS derived_id
 FROM requested r
 LEFT JOIN observation_occurrences o ON r.kind='occurrence' AND o.subject_id=$1 AND o.occurrence_id=r.id AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 LEFT JOIN artifacts a ON r.kind='artifact' AND a.subject_id=$1 AND a.artifact_id=r.id
 LEFT JOIN source_regions sr ON r.kind='source_region' AND sr.subject_id=$1 AND sr.source_region_id=r.id AND ($4::timestamptz IS NULL OR sr.created_at<=$4)
 LEFT JOIN derived_representations d ON r.kind='derived_representation' AND d.subject_id=$1 AND d.derived_representation_id=r.id
 LEFT JOIN derived_regions dr ON r.kind='derived_region' AND dr.subject_id=$1 AND dr.derived_region_id=r.id
), sources AS (
 SELECT ordinality,artifact_id FROM resolved WHERE artifact_id IS NOT NULL
 UNION SELECT r.ordinality,sr.artifact_id FROM resolved r CROSS JOIN LATERAL representation_source_regions($1,r.derived_id) roots JOIN source_regions sr USING(source_region_id)
 WHERE sr.subject_id=$1 AND ($4::timestamptz IS NULL OR sr.created_at<=$4)
), lineage AS (
 SELECT s.artifact_id AS origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref FROM (SELECT DISTINCT artifact_id FROM sources) s
 JOIN observation_occurrences o ON o.subject_id=$1 AND o.artifact_id=s.artifact_id AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 UNION SELECT p.origin,o.occurrence_id,o.artifact_id,o.source_class,o.external_object_ref FROM lineage p
 JOIN observation_occurrences o ON (o.artifact_id IS NOT NULL AND o.artifact_id=p.artifact_id) OR (o.external_object_ref IS NOT NULL AND o.external_object_ref=p.external_object_ref AND o.source_class=p.source_class)
 WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
), external_roots AS (
 SELECT origin,MIN('external:'||source_class||':'||external_object_ref) AS root FROM lineage WHERE external_object_ref IS NOT NULL GROUP BY origin
)
SELECT r.ordinality,r.present,r.derived_id,r.source_class,r.external_object_ref,
 ARRAY(SELECT DISTINCT s.artifact_id FROM sources s WHERE s.ordinality=r.ordinality) AS artifacts,
 ARRAY(SELECT DISTINCT coalesce(e.root,'artifact:'||s.artifact_id) FROM sources s LEFT JOIN external_roots e ON e.origin=s.artifact_id WHERE s.ordinality=r.ordinality) AS roots
FROM resolved r
"#;

impl MaterialService {
    /// External object origins are source facts, separate from independence-root grouping.
    pub async fn source_objects_batch(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, Vec<CognitiveRef>>> {
        if references.len() > 256 || view.is_some_and(|v| v.subject != subject) {
            return Err(Error::Invalid("source object batch scope exceeded".into()));
        }
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) r(kind,value)), origins AS (
 SELECT r.kind,r.value,o.external_object_ref FROM requested r JOIN observation_occurrences o ON r.kind='occurrence' AND o.occurrence_id::text=r.value WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
 UNION
 SELECT r.kind,r.value,o.external_object_ref FROM requested r
 LEFT JOIN derived_regions g ON r.kind='derived_region' AND g.derived_region_id::text=r.value AND g.subject_id=$1
 JOIN derived_representations d ON d.subject_id=$1 AND ((r.kind='derived_representation' AND d.derived_representation_id::text=r.value) OR d.derived_representation_id=g.derived_representation_id)
 CROSS JOIN LATERAL representation_source_regions($1,d.derived_representation_id) roots JOIN source_regions s USING(source_region_id)
 JOIN observation_occurrences o ON o.subject_id=$1 AND o.artifact_id=s.artifact_id
 WHERE ($4::timestamptz IS NULL OR (d.created_at<=$4 AND s.created_at<=$4 AND o.created_at<=$4))
) SELECT DISTINCT kind,value,external_object_ref FROM origins WHERE external_object_ref IS NOT NULL ORDER BY kind,value,external_object_ref
"#).bind(subject.0).bind(kinds).bind(values).bind(view.map(|v| v.as_of)).fetch_all(self.store.pool()).await.map_err(db)?;
        let mut result = HashMap::<CognitiveRef, Vec<CognitiveRef>>::new();
        for row in rows {
            let reference = parse_reference(
                &row.try_get::<String, _>("kind").map_err(db)?,
                &row.try_get::<String, _>("value").map_err(db)?,
            )?;
            result
                .entry(reference)
                .or_default()
                .push(CognitiveRef::ExternalObject(ObjectRef::new(
                    row.try_get::<String, _>("external_object_ref")
                        .map_err(db)?,
                )?));
        }
        Ok(result)
    }

    pub async fn evidence_roots(
        &self,
        subject: SubjectId,
        evidence: &EvidenceRef,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<BTreeSet<EvidenceRoot>> {
        self.evidence_roots_batch(subject, std::slice::from_ref(evidence), view)
            .await?
            .pop()
            .ok_or_else(|| Error::Infrastructure("evidence roots result missing".into()))
    }

    /// Qualify each locator against its actual occurrence artifact and resolve all roots once.
    pub async fn evidence_roots_batch(
        &self,
        subject: SubjectId,
        evidence: &[EvidenceRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<BTreeSet<EvidenceRoot>>> {
        let occurrences = self
            .occurrences(
                subject,
                &evidence
                    .iter()
                    .map(|evidence| evidence.occurrence_id)
                    .collect::<Vec<_>>(),
                view,
            )
            .await?;
        let references: Vec<_> = evidence.iter().map(evidence_reference).collect();
        let roots = self.lineage_entries(subject, &references, view).await?;
        evidence
            .iter()
            .zip(references)
            .map(|(evidence, reference)| {
                let entry = &roots[&reference];
                let occurrence = &occurrences[&evidence.occurrence_id];
                if !matches!(evidence.locator, EvidenceLocator::WholeOccurrence)
                    && !occurrence
                        .artifact_id
                        .is_some_and(|artifact| entry.artifacts.contains(&artifact.0))
                {
                    return Err(Error::Invalid(
                        "evidence locator source does not match occurrence".into(),
                    ));
                }
                Ok(entry.roots.clone())
            })
            .collect()
    }

    pub async fn lineage_roots_batch(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, BTreeSet<EvidenceRoot>>> {
        Ok(self
            .lineage_entries(subject, references, view)
            .await?
            .into_iter()
            .map(|(reference, lineage)| (reference, lineage.roots))
            .collect())
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

    async fn lineage_entries(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, Lineage>> {
        if references.len() > 256 {
            return Err(Error::Invalid(
                "Material lineage batch exceeds 256 references".into(),
            ));
        }
        if references.is_empty() {
            return Ok(HashMap::new());
        }
        let references: Vec<_> = references
            .iter()
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut kinds = Vec::new();
        let mut ids = Vec::new();
        for reference in &references {
            self.require_lineage_view(subject, reference, view)?;
            let (kind, id) = match reference {
                CognitiveRef::Occurrence(id) => ("occurrence", id.0),
                CognitiveRef::Artifact(id) => ("artifact", id.0),
                CognitiveRef::SourceRegion(id) => ("source_region", id.0),
                CognitiveRef::DerivedRepresentation(id) => ("derived_representation", id.0),
                CognitiveRef::DerivedRegion(id) => ("derived_region", id.0),
                _ => {
                    return Err(Error::Invalid(
                        "lineage requires a Material reference".into(),
                    ));
                }
            };
            kinds.push(kind);
            ids.push(id);
        }
        let rows = sqlx::query(LINEAGE_READ)
            .bind(subject.0)
            .bind(kinds)
            .bind(ids)
            .bind(view.map(|view| view.as_of))
            .fetch_all(self.store.pool())
            .await
            .map_err(db)?;
        let mut result = HashMap::new();
        for row in rows {
            let ordinal: i64 = row.try_get("ordinality").map_err(db)?;
            let reference = &references[ordinal as usize - 1];
            if !row.try_get::<bool, _>("present").map_err(db)? {
                return Err(Error::NotFound(
                    "Material lineage reference is outside Subject/view".into(),
                ));
            }
            let artifacts: Vec<Uuid> = row.try_get("artifacts").map_err(db)?;
            let keys: Vec<String> = row.try_get("roots").map_err(db)?;
            let mut roots: BTreeSet<_> = keys
                .into_iter()
                .map(|root_key| EvidenceRoot {
                    root_key,
                    certainty: EvidenceRootCertainty::Known,
                })
                .collect();
            if roots.is_empty() {
                if row
                    .try_get::<Option<Uuid>, _>("derived_id")
                    .map_err(db)?
                    .is_some()
                {
                    return Err(Error::Invalid(
                        "derived evidence has no valid source roots".into(),
                    ));
                }
                let external: Option<String> = row.try_get("external_object_ref").map_err(db)?;
                let root = match external {
                    Some(external) => EvidenceRoot {
                        root_key: format!(
                            "external:{}:{external}",
                            row.try_get::<String, _>("source_class").map_err(db)?
                        ),
                        certainty: EvidenceRootCertainty::Known,
                    },
                    None => EvidenceRoot {
                        root_key: format!("occurrence:{}", reference_parts(reference).1),
                        certainty: EvidenceRootCertainty::OccurrenceOnly,
                    },
                };
                roots.insert(root);
            }
            result.insert(reference.clone(), Lineage { roots, artifacts });
        }
        Ok(result)
    }
}
