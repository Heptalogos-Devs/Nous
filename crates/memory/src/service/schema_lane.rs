// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::lane::{LaneCandidate, LaneOutput, LaneStatus};
use super::*;
use nous_runtime::{BoundQuery, QueryPlan};
use sqlx::Row;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[expect(
    clippy::too_many_lines,
    reason = "schema direct lane batches schema and support projection semantics"
)]
pub(crate) async fn schema_direct_lane(
    service: &MemoryService,
    bound: &BoundQuery,
    plan: &QueryPlan,
) -> Result<LaneOutput> {
    let query = &bound.source_query;
    let mut tx = service.store.read_view().await?;
    let mut schema_ids = BTreeSet::new();
    let mut exact_revisions = BTreeSet::new();
    for cue in &query.expression.cues {
        if let Cue::Schema(value) = cue {
            schema_ids.insert(value.schema.0);
        }
    }
    for target in &query.expression.targets {
        if let QueryTarget::SchemaNeighborhood { schema } = target {
            schema_ids.insert(schema.0);
        }
    }
    for binding in &bound.exact_bindings {
        match binding.bound_ref {
            CognitiveRef::CognitiveSchema(schema) => {
                schema_ids.insert(schema.0);
            }
            CognitiveRef::CognitiveSchemaRevision(revision) => {
                exact_revisions.insert(revision.0);
            }
            _ => {}
        }
    }
    let mut output = LaneOutput::empty(EvidenceFamily::SchemaDirect, LaneStatus::Ready);
    if schema_ids.is_empty() && exact_revisions.is_empty() {
        return Ok(output);
    }
    let rows = sqlx::query(
        "SELECT s.schema_id,s.current_revision_id,s.object_epoch,s.acceptance_state,s.integrity_state,s.suppression_state,r.schema_id,r.schema_revision_id,r.aboutness,r.valid_time_domain_ref,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.formed_time_domain_ref,r.formed_recorded_at,r.recorded_at FROM cognitive_schemas s JOIN cognitive_schema_revisions r ON r.schema_id=s.schema_id WHERE s.subject_id=$1 AND ((s.schema_id=ANY($2::uuid[]) AND r.schema_revision_id=s.current_revision_id) OR r.schema_revision_id=ANY($3::uuid[])) AND ($4::text[] IS NULL OR source_read_allowed($1,'cognitive_schema_revision',r.schema_revision_id::text,$4,$5,$6,$7)) ORDER BY (r.schema_revision_id=ANY($3::uuid[])) DESC,r.schema_revision_id LIMIT $8",
    )
    .bind(query.subject.0)
    .bind(schema_ids.iter().copied().collect::<Vec<_>>())
    .bind(exact_revisions.iter().copied().collect::<Vec<_>>()).bind(query.source_read.as_ref().map(|v|v.root_keys.clone())).bind(query.source_read.as_ref().map(|v|v.work_contexts.clone()).unwrap_or_default()).bind(query.source_read.as_ref().map(|v|v.origin_conversation_refs.clone()).unwrap_or_default()).bind(query.source_read.as_ref().map(|v|serde_json::json!(&v.explicit_refs)).unwrap_or_else(||serde_json::json!([]))).bind((plan.lane_budget(EvidenceFamily::SchemaDirect)+1) as i64)
    .fetch_all(&mut *tx)
    .await
    .map_err(nous_persistence::database_error)?;
    if rows.len() > plan.lane_budget(EvidenceFamily::SchemaDirect) {
        output.status = LaneStatus::Truncated;
    }
    let mut rows = rows
        .into_iter()
        .take(plan.lane_budget(EvidenceFamily::SchemaDirect))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        let left_id: Uuid = left.try_get("schema_revision_id").unwrap_or_default();
        let right_id: Uuid = right.try_get("schema_revision_id").unwrap_or_default();
        let left_exact = exact_revisions.contains(&left_id);
        let right_exact = exact_revisions.contains(&right_id);
        right_exact
            .cmp(&left_exact)
            .then_with(|| left_id.cmp(&right_id))
    });
    let revision_keys = rows
        .iter()
        .filter_map(|row| row.try_get::<Uuid, _>("schema_revision_id").ok())
        .collect::<Vec<_>>();
    let mut metadata = super::longitudinal_query::longitudinal_metadata(
        &mut tx,
        query.subject,
        "cognitive_schema",
        &revision_keys,
        bound.historical_authority.as_deref(),
        bound.source_query.source_read.as_ref(),
    )
    .await?;
    let mut revision_ids = Vec::new();
    for (index, row) in rows.into_iter().enumerate() {
        let revision = CognitiveSchemaRevisionId(
            row.try_get("schema_revision_id")
                .map_err(nous_persistence::database_error)?,
        );
        revision_ids.push(revision.0);
        let aboutness = row
            .try_get::<Vec<String>, _>("aboutness")
            .map_err(nous_persistence::database_error)?
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<Vec<_>>>()?;
        let historical = bound
            .revision_policy
            .allows_historical(&CognitiveRef::CognitiveSchemaRevision(revision));
        let accepted = row
            .try_get::<String, _>("acceptance_state")
            .map_err(nous_persistence::database_error)?
            == "accepted";
        let valid = row
            .try_get::<String, _>("integrity_state")
            .map_err(nous_persistence::database_error)?
            == "valid";
        let normal = row
            .try_get::<String, _>("suppression_state")
            .map_err(nous_persistence::database_error)?
            == "normal";
        let facts = metadata.remove(&revision.0).unwrap_or_default();
        let constraints_match = query.expression.constraints.matches_common(QueryFacts {
            authority: AuthorityClass::SubjectCognition,
            entities: &aboutness,
            source_classes: &facts.source_classes,
            modality: Modality::Text,
            cognitive_role: None,
            formation_mode: None,
            epistemic_class: None,
        });
        let current = row
            .try_get::<Uuid, _>("current_revision_id")
            .map_err(nous_persistence::database_error)?
            == revision.0;
        if (historical || (accepted && valid && normal && current)) && constraints_match {
            let reference = CognitiveRef::CognitiveSchemaRevision(revision);
            output.candidates.push(LaneCandidate {
                reference: reference.clone(),
                rank: (index + 1) as u32,
                variants: vec!["schema:direct".into()],
                provider_metadata: serde_json::Value::Null,
            });
        }
    }
    if !revision_ids.is_empty() {
        let rows = sqlx::query(
            "SELECT basis_kind,basis_ref FROM cognitive_schema_evidence_links WHERE subject_id=$1 AND role='support' AND schema_revision_id=ANY($2::uuid[]) AND revoked_at IS NULL ORDER BY basis_kind,basis_ref",
        )
        .bind(query.subject.0)
        .bind(revision_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;
        let rank = (output.candidates.len() + 1) as u32;
        for row in rows {
            let kind: String = row
                .try_get("basis_kind")
                .map_err(nous_persistence::database_error)?;
            if let Ok(reference) = parse_reference(
                &kind,
                &row.try_get::<String, _>("basis_ref")
                    .map_err(nous_persistence::database_error)?,
            ) {
                output.candidates.push(LaneCandidate {
                    reference,
                    rank,
                    variants: vec!["schema:direct_support".into()],
                    provider_metadata: serde_json::Value::Null,
                });
            }
        }
    }
    Ok(output)
}

#[expect(
    clippy::too_many_lines,
    reason = "Schema owner batch materialization keeps lifecycle, constraints, and payload aligned"
)]
pub(crate) async fn materialize_schema_revisions(
    service: &MemoryService,
    bound: &BoundQuery,
    references: &[CognitiveRef],
) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)> {
    let mut tx = service.store.read_view().await?;
    let mut admitted = Vec::new();
    for reference in references {
        if bound.source_query.source_read.is_none()
            || AuthorityStore::source_read_allowed_in(
                &mut tx,
                bound
                    .source_query
                    .source_read
                    .as_ref()
                    .expect("checked read"),
                reference,
            )
            .await?
        {
            admitted.push(reference.clone());
        }
    }
    let references = admitted.as_slice();
    let revision_ids = references
        .iter()
        .filter_map(|reference| match reference {
            CognitiveRef::CognitiveSchemaRevision(value) => Some(value.0),
            _ => None,
        })
        .collect::<Vec<_>>();
    if revision_ids.is_empty() {
        return Ok((Vec::new(), BTreeMap::new()));
    }
    let rows = sqlx::query("SELECT s.current_revision_id,s.object_epoch,s.acceptance_state,s.integrity_state,s.suppression_state,r.schema_id,r.schema_revision_id,r.structural_claim,r.aboutness,r.valid_time_domain_ref,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.formed_time_domain_ref,r.formed_recorded_at,r.recorded_at FROM cognitive_schemas s JOIN cognitive_schema_revisions r ON r.schema_id=s.schema_id WHERE s.subject_id=$1 AND r.schema_revision_id=ANY($2::uuid[])")
        .bind(bound.source_query.subject.0)
        .bind(&revision_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(nous_persistence::database_error)?;
    let mut metadata = super::longitudinal_query::longitudinal_metadata(
        &mut tx,
        bound.source_query.subject,
        "cognitive_schema",
        &revision_ids,
        bound.historical_authority.as_deref(),
        bound.source_query.source_read.as_ref(),
    )
    .await?;
    let mut drops = BTreeMap::new();
    let mut hits = Vec::new();
    for row in rows {
        let revision = CognitiveSchemaRevisionId(
            row.try_get("schema_revision_id")
                .map_err(nous_persistence::database_error)?,
        );
        let reference = CognitiveRef::CognitiveSchemaRevision(revision);
        let header = match super::historical::historical_header(bound, &reference) {
            Ok(header) => header,
            Err(Error::NotFound(_)) => {
                *drops.entry("outside_historical_view".into()).or_default() += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        let historical = bound.revision_policy.allows_historical(&reference);
        let current: Uuid = row
            .try_get("current_revision_id")
            .map_err(nous_persistence::database_error)?;
        if !historical && current != revision.0 {
            *drops.entry("not_current".into()).or_default() += 1;
            continue;
        }
        let accepted = super::historical::header_text(header, &row, "acceptance_state")?;
        let valid = super::historical::header_text(header, &row, "integrity_state")?;
        let suppression = super::historical::header_text(header, &row, "suppression_state")?;
        if accepted != "accepted" || valid != "valid" || suppression != "normal" {
            *drops.entry("unavailable_lifecycle".into()).or_default() += 1;
            continue;
        }
        if let Some(binding) = bound
            .exact_bindings
            .iter()
            .find(|binding| binding.bound_ref == reference)
            && binding.mutable_object
            && binding.bound_object_epoch != Some(super::historical::header_epoch(header, &row)?)
        {
            *drops.entry("stale_exact_binding".into()).or_default() += 1;
            continue;
        }
        let aboutness = row
            .try_get::<Vec<String>, _>("aboutness")
            .map_err(nous_persistence::database_error)?
            .into_iter()
            .map(EntityRef::new)
            .collect::<Result<Vec<_>>>()?;
        let source_times = metadata.remove(&revision.0).unwrap_or_default();
        if !bound
            .source_query
            .expression
            .constraints
            .matches_common(QueryFacts {
                authority: AuthorityClass::SubjectCognition,
                entities: &aboutness,
                source_classes: &source_times.source_classes,
                modality: Modality::Text,
                cognitive_role: None,
                formation_mode: None,
                epistemic_class: None,
            })
        {
            *drops.entry("query_constraints".into()).or_default() += 1;
            continue;
        }
        let freshness = FreshnessDescriptor {
            temporal_evidence: source_times.temporal,
            valid_time: DomainTemporalExtent {
                time_domain_ref: row
                    .try_get("valid_time_domain_ref")
                    .map_err(nous_persistence::database_error)?,
                extent: temporal_from_columns(
                    row.try_get("valid_time_kind")
                        .map_err(nous_persistence::database_error)?,
                    row.try_get("valid_time_start")
                        .map_err(nous_persistence::database_error)?,
                    row.try_get("valid_time_end")
                        .map_err(nous_persistence::database_error)?,
                )?,
            },
            formed_at: Some(DomainStamp {
                time_domain_ref: row
                    .try_get("formed_time_domain_ref")
                    .map_err(nous_persistence::database_error)?,
                at: row
                    .try_get("formed_at")
                    .map_err(nous_persistence::database_error)?,
                recorded_at: row
                    .try_get("formed_recorded_at")
                    .map_err(nous_persistence::database_error)?,
            }),
            recorded_at: Some(
                row.try_get("recorded_at")
                    .map_err(nous_persistence::database_error)?,
            ),
        };
        let constraints = &bound.source_query.expression.constraints;
        if !freshness.temporal_evidence.matches(
            bound
                .source_query
                .temporal_predicate(TimeAxis::Occurred)
                .as_ref(),
            bound
                .source_query
                .temporal_predicate(TimeAxis::Observed)
                .as_ref(),
        ) || !bound
            .source_query
            .temporal_predicate(TimeAxis::Valid)
            .is_none_or(|p| p.matches_extent(&freshness.valid_time) == Some(true))
            || !bound
                .source_query
                .temporal_predicate(TimeAxis::Formed)
                .is_none_or(|p| {
                    freshness
                        .formed_at
                        .as_ref()
                        .is_some_and(|stamp| p.matches_stamp(stamp) == Some(true))
                })
            || !constraints
                .recorded
                .is_none_or(|p| freshness.recorded_at.is_some_and(|at| p.contains(at)))
        {
            *drops.entry("temporal_ineligible".into()).or_default() += 1;
            continue;
        }
        hits.push(CognitiveHit {
            authority_epoch: Some(super::historical::header_epoch(header, &row)?),
            preference_refs: vec![CognitiveRef::CognitiveSchema(CognitiveSchemaId(
                row.try_get("schema_id")
                    .map_err(nous_persistence::database_error)?,
            ))],
            reference: reference.clone(),
            revision: None,
            semantic_role: Some("cognitive_schema".into()),
            cognitive_role: None,
            formation_mode: None,
            representation: Some(
                row.try_get("structural_claim")
                    .map_err(nous_persistence::database_error)?,
            ),
            authority: AuthorityClass::SubjectCognition,
            freshness,
            entity_refs: aboutness,
            evidence: Vec::new(),
            match_evidence: MatchEvidence::default(),
            materialization: Vec::new(),
        });
    }
    Ok((hits, drops))
}
