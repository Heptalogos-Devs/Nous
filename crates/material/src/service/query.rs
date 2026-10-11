//! Evidence time metadata, resolved from the actual Subject-scoped sources.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use chrono::{DateTime, Utc};
use nous_core::*;
use nous_persistence::database_error;
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Default)]
struct EvidenceTimes {
    pub sources: Vec<(TemporalExtent, DateTime<Utc>, DateTime<Utc>)>,
    pub formed_at: Option<DateTime<Utc>>,
    source_classes: Vec<SourceClass>,
}
impl EvidenceTimes {
    pub fn matches(&self, constraints: &QueryConstraints) -> bool {
        if constraints.valid.is_some() {
            return false;
        } // Evidence has no world-valid claim.
        if constraints
            .formed
            .is_some_and(|interval| self.formed_at.is_none_or(|at| !interval.contains(at)))
        {
            return false;
        }
        let filtered = constraints.occurred.is_some()
            || constraints.observed.is_some()
            || constraints.recorded.is_some();
        !filtered
            || self.sources.iter().any(|(occurred, observed, recorded)| {
                constraints
                    .occurred
                    .is_none_or(|interval| interval.matches_extent(occurred))
                    && constraints
                        .observed
                        .is_none_or(|interval| interval.contains(*observed))
                    && constraints
                        .recorded
                        .is_none_or(|interval| interval.contains(*recorded))
            })
    }
    pub fn freshness(&self) -> FreshnessDescriptor {
        FreshnessDescriptor {
            occurred: self
                .sources
                .iter()
                .map(|(time, _, _)| time.clone())
                .collect(),
            observed_at: self.sources.iter().map(|(_, at, _)| *at).max(),
            recorded_at: self.sources.iter().map(|(_, _, at)| *at).max(),
            formed_at: self.formed_at,
            valid_time: TemporalExtent::Unknown,
        }
    }
}
impl MaterialService {
    async fn evidence_times_in_view(
        &self,
        subject: SubjectId,
        refs: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<CognitiveRef, EvidenceTimes>> {
        if refs.len() > 256 {
            return Err(Error::Invalid("reference time envelope exceeded".into()));
        }
        let mut kinds = Vec::new();
        let mut values = Vec::new();
        let mut result = HashMap::new();
        for reference in refs {
            let text = reference.to_string();
            let (kind, value) = text
                .split_once(':')
                .ok_or_else(|| Error::Invalid("invalid time reference".into()))?;
            kinds.push(kind.to_owned());
            values.push(value.to_owned());
            result.insert(reference.clone(), EvidenceTimes::default());
        }
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) AS r(kind,value)),
origins AS (
 SELECT r.kind,r.value,o.occurrence_id,NULL::timestamptz formed_at,o.created_at recorded_at FROM requested r JOIN observation_occurrences o ON r.kind='occurrence' AND o.occurrence_id::text=r.value AND o.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,o.occurrence_id,NULL::timestamptz,s.created_at FROM requested r JOIN source_regions s ON r.kind='source_region' AND s.source_region_id::text=r.value AND s.subject_id=$1 JOIN observation_occurrences o ON o.artifact_id=s.artifact_id AND o.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,o.occurrence_id,d.created_at,COALESCE(g.created_at,d.created_at) FROM requested r JOIN derived_representations d ON d.subject_id=$1 AND (r.kind='derived_representation' AND d.derived_representation_id::text=r.value OR r.kind='derived_region' AND EXISTS(SELECT 1 FROM derived_regions g WHERE g.subject_id=$1 AND g.derived_region_id::text=r.value AND g.derived_representation_id=d.derived_representation_id))
 LEFT JOIN derived_regions g ON r.kind='derived_region' AND g.subject_id=$1 AND g.derived_region_id::text=r.value
 JOIN LATERAL representation_source_regions($1,d.derived_representation_id) roots ON true JOIN source_regions s USING(source_region_id) JOIN observation_occurrences o ON o.artifact_id=s.artifact_id AND o.subject_id=$1
)
SELECT DISTINCT p.kind,p.value,p.formed_at,o.occurrence_id,o.occurred_time_kind,o.occurred_time_start,o.occurred_time_end,o.observed_at,p.recorded_at,o.source_class FROM origins p JOIN observation_occurrences o USING(occurrence_id) WHERE o.subject_id=$1 AND ($4::timestamptz IS NULL OR o.created_at<=$4)
"#).bind(subject.0).bind(&kinds).bind(&values).bind(view.map(|v|v.as_of)).fetch_all(self.store.pool()).await.map_err(database_error)?;
        for row in rows {
            let reference = parse_reference(
                &row.get::<String, _>("kind"),
                &row.get::<String, _>("value"),
            )?;
            let kind: String = row.get("occurred_time_kind");
            let start: Option<DateTime<Utc>> = row.get("occurred_time_start");
            let end = row.get("occurred_time_end");
            let occurred = match kind.as_str() {
                "instant" => TemporalExtent::Instant {
                    at: start.ok_or_else(|| {
                        Error::Infrastructure("instant occurrence has no time".into())
                    })?,
                },
                "interval" => TemporalExtent::Interval { start, end },
                _ => TemporalExtent::Unknown,
            };
            let times = result
                .get_mut(&reference)
                .ok_or_else(|| Error::Infrastructure("unexpected reference time row".into()))?;
            times
                .sources
                .push((occurred, row.get("observed_at"), row.get("recorded_at")));
            times.formed_at = row.get("formed_at");
            times
                .source_classes
                .push(SourceClass::from(row.get::<String, _>("source_class")));
        }
        Ok(result)
    }
}

fn material_hit(
    reference: CognitiveRef,
    family: EvidenceFamily,
    query: &CognitiveQuery,
) -> CognitiveHit {
    let authority = if matches!(
        reference,
        CognitiveRef::DerivedRepresentation(_) | CognitiveRef::DerivedRegion(_)
    ) {
        AuthorityClass::Interpretation
    } else {
        AuthorityClass::Evidence
    };
    CognitiveHit {
        reference: reference.clone(),
        revision: None,
        authority_epoch: None,
        preference_refs: Vec::new(),
        semantic_role: Some("material".into()),
        cognitive_role: None,
        formation_mode: None,
        representation: None,
        authority,
        freshness: FreshnessDescriptor {
            occurred: Vec::new(),
            observed_at: None,
            valid_time: TemporalExtent::Unknown,
            formed_at: None,
            recorded_at: None,
        },
        entity_refs: Vec::new(),
        evidence: if query.result_need.need_evidence {
            vec![EvidenceHandle {
                epistemic_relation: None,
                reference: reference.clone(),
                basis_role: "source".into(),
            }]
        } else {
            Vec::new()
        },
        match_evidence: MatchEvidence {
            families: vec![family],
            ..Default::default()
        },
        materialization: if query.result_need.need_materialization_handles {
            vec![MaterializationHandle {
                reference,
                level: "source".into(),
            }]
        } else {
            Vec::new()
        },
    }
}

#[async_trait::async_trait]
impl nous_runtime::CognitiveContributor for MaterialService {
    fn owns(&self, reference: &CognitiveRef) -> bool {
        matches!(
            reference,
            CognitiveRef::Artifact(_)
                | CognitiveRef::Occurrence(_)
                | CognitiveRef::SourceRegion(_)
                | CognitiveRef::DerivedRepresentation(_)
                | CognitiveRef::DerivedRegion(_)
        )
    }
    async fn direct_lanes(
        &self,
        bound: &nous_runtime::BoundQuery,
        plan: &nous_runtime::QueryPlan,
    ) -> Result<Vec<nous_runtime::LaneOutput>> {
        use nous_runtime::{LaneCandidate, LaneOutput, LaneStatus};
        let c = &bound.source_query.expression.constraints;
        if bound
            .source_query
            .expression
            .targets
            .iter()
            .any(|target| matches!(target, QueryTarget::Exact { .. }))
            || !bound.lane_enabled(EvidenceFamily::Temporal)
            || (!bound.source_query.projection.domain_names().is_empty()
                && !bound
                    .source_query
                    .projection
                    .domain_names()
                    .contains(&"evidence"))
            || [c.occurred, c.observed, c.recorded]
                .iter()
                .all(Option::is_none)
        {
            return Ok(Vec::new());
        }
        let mut output = LaneOutput::empty(EvidenceFamily::Temporal, LaneStatus::Ready);
        if c.valid.is_some() || c.formed.is_some() {
            return Ok(vec![output]);
        }
        let mut sql = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT occurrence_id FROM observation_occurrences WHERE subject_id=",
        );
        sql.push_bind(bound.source_query.subject.0);
        if let Some(view) = bound.historical_authority.as_deref() {
            let ids: Vec<_> = view
                .material_documents
                .iter()
                .filter_map(|reference| match reference {
                    CognitiveRef::Occurrence(id) => Some(id.0),
                    _ => None,
                })
                .collect();
            sql.push(" AND occurrence_id=ANY(").push_bind(ids).push(")");
        }
        use nous_persistence::{TimeColumns, push_time_predicate};
        if let Some(predicate) = c.occurred {
            push_time_predicate(
                &mut sql,
                TimeColumns::Extent {
                    kind: "occurred_time_kind",
                    start: "occurred_time_start",
                    end: "occurred_time_end",
                },
                predicate,
            )?;
        }
        for (column, predicate) in [("observed_at", c.observed), ("created_at", c.recorded)] {
            if let Some(predicate) = predicate {
                push_time_predicate(&mut sql, TimeColumns::Instant(column), predicate)?;
            }
        }
        if !c.source_classes_include.is_empty() {
            sql.push(" AND source_class=ANY(")
                .push_bind(
                    c.source_classes_include
                        .iter()
                        .map(|v| v.as_str().to_owned())
                        .collect::<Vec<_>>(),
                )
                .push(")");
        }
        if !c.source_classes_exclude.is_empty() {
            sql.push(" AND source_class<>ALL(")
                .push_bind(
                    c.source_classes_exclude
                        .iter()
                        .map(|v| v.as_str().to_owned())
                        .collect::<Vec<_>>(),
                )
                .push(")");
        }
        let limit = plan.lane_budget(EvidenceFamily::Temporal);
        sql.push(" ORDER BY observed_at DESC,occurrence_id LIMIT ")
            .push_bind((limit + 1) as i64);
        let ids: Vec<Uuid> = sql
            .build_query_scalar()
            .fetch_all(self.store.pool())
            .await
            .map_err(database_error)?;
        if ids.len() > limit {
            output.status = LaneStatus::Truncated;
        }
        output.candidates = ids
            .into_iter()
            .take(limit)
            .enumerate()
            .map(|(index, id)| LaneCandidate {
                reference: CognitiveRef::Occurrence(OccurrenceId(id)),
                rank: (index + 1) as u32,
                variants: Vec::new(),
                provider_metadata: serde_json::Value::Null,
            })
            .collect();
        Ok(vec![output])
    }
    async fn validate_and_materialize(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        bound: &nous_runtime::BoundQuery,
    ) -> Result<(Vec<CognitiveHit>, std::collections::BTreeMap<String, usize>)> {
        let query = &bound.source_query;
        if subject != query.subject {
            return Err(Error::Invalid("material query subject mismatch".into()));
        }
        let visible: Vec<_> = references
            .iter()
            .filter(|reference| {
                bound
                    .historical_authority
                    .as_ref()
                    .is_none_or(|view| view.material_visibility.contains(reference))
            })
            .cloned()
            .collect();
        let references = visible.as_slice();
        let mut times = self
            .evidence_times_in_view(subject, references, bound.historical_authority.as_deref())
            .await?;
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query(r#"
WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) AS r(kind,value)),
records AS (
 SELECT r.kind,r.value,a.created_at recorded_at,NULL::timestamptz formed_at,a.media_type,NULL::text payload_text FROM requested r JOIN artifacts a ON r.kind='artifact' AND a.artifact_id::text=r.value AND a.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,o.created_at,NULL::timestamptz,COALESCE(a.media_type,'application/octet-stream'),NULL::text FROM requested r JOIN observation_occurrences o ON r.kind='occurrence' AND o.occurrence_id::text=r.value AND o.subject_id=$1 LEFT JOIN artifacts a ON a.artifact_id=o.artifact_id AND a.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,s.created_at,NULL::timestamptz,a.media_type,NULL::text FROM requested r JOIN source_regions s ON r.kind='source_region' AND s.source_region_id::text=r.value AND s.subject_id=$1 JOIN artifacts a ON a.artifact_id=s.artifact_id AND a.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,d.created_at,d.created_at,'text/plain',d.payload_text FROM requested r JOIN derived_representations d ON r.kind='derived_representation' AND d.derived_representation_id::text=r.value AND d.subject_id=$1
 UNION ALL
 SELECT r.kind,r.value,g.created_at,d.created_at,'text/plain',NULL::text FROM requested r JOIN derived_regions g ON r.kind='derived_region' AND g.derived_region_id::text=r.value AND g.subject_id=$1 JOIN derived_representations d ON d.derived_representation_id=g.derived_representation_id AND d.subject_id=$1
)
SELECT * FROM records
"#).bind(subject.0).bind(kinds).bind(values).fetch_all(self.store.pool()).await.map_err(database_error)?;
        let mut hits = Vec::new();
        let mut drops = std::collections::BTreeMap::new();
        for row in rows {
            let reference = parse_reference(
                &row.get::<String, _>("kind"),
                &row.get::<String, _>("value"),
            )?;
            let metadata = times
                .get_mut(&reference)
                .ok_or_else(|| Error::Infrastructure("unexpected Material query record".into()))?;
            let recorded_at: DateTime<Utc> = row.get("recorded_at");
            metadata.formed_at = row.get("formed_at");
            let c = &query.expression.constraints;
            let mut source_constraints = c.clone();
            source_constraints.recorded = None;
            let mut hit = material_hit(reference, EvidenceFamily::Exact, query);
            let media: String = row.get("media_type");
            let modality = if media.starts_with("text/") {
                Modality::Text
            } else if media.starts_with("image/") {
                Modality::Image
            } else if media.starts_with("audio/") {
                Modality::Audio
            } else if media.starts_with("video/") {
                Modality::Video
            } else if media.contains("json") {
                Modality::Structured
            } else {
                Modality::Binary
            };
            if !c.matches_common(QueryFacts {
                authority: hit.authority,
                entities: &[],
                source_classes: &metadata.source_classes,
                modality,
                cognitive_role: None,
                formation_mode: None,
                epistemic_class: None,
            }) || !metadata.matches(&source_constraints)
                || c.recorded
                    .is_some_and(|predicate| !predicate.contains(recorded_at))
            {
                *drops
                    .entry("material_constraint_ineligible".into())
                    .or_default() += 1;
                continue;
            }
            hit.freshness = metadata.freshness();
            hit.freshness.recorded_at = Some(recorded_at);
            hit.representation = row
                .get::<Option<String>, _>("payload_text")
                .map(|text| text.chars().take(8192).collect());
            hits.push(hit);
        }
        let missing = references
            .len()
            .saturating_sub(hits.len() + drops.values().sum::<usize>());
        if missing > 0 {
            drops.insert("material_not_in_subject".into(), missing);
        }
        Ok((hits, drops))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/query.rs"]
mod tests;
