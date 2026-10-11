// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::BoundQuery;
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn materialize_longitudinal(
    service: &MemoryService,
    bound: &BoundQuery,
    references: &[CognitiveRef],
) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)> {
    let subject = bound.source_query.subject;
    let mut objects = BTreeMap::<&str, Vec<Uuid>>::new();
    let mut revisions = BTreeMap::<&str, Vec<Uuid>>::new();
    for reference in references {
        match reference {
            CognitiveRef::Episode(id) => objects.entry("episode").or_default().push(id.0),
            CognitiveRef::Journal(id) => objects.entry("journal").or_default().push(id.0),
            CognitiveRef::EpisodeRevision(id) => revisions.entry("episode").or_default().push(id.0),
            CognitiveRef::JournalRevision(id) => revisions.entry("journal").or_default().push(id.0),
            _ => (),
        }
    }
    let mut hits = Vec::new();
    let mut drops = BTreeMap::new();
    for (kind, query) in [
        (
            "episode",
            "SELECT o.episode_id AS object_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,r.episode_revision_id AS revision_id,r.title,r.boundary_explanation AS text,r.experience_time_kind AS time_kind,r.experience_time_start AS time_start,r.experience_time_end AS time_end,r.formed_at,r.recorded_at FROM episode_objects o JOIN episode_revisions r USING(episode_id) WHERE o.subject_id=$1 AND ((o.episode_id=ANY($2::uuid[]) AND r.episode_revision_id=o.current_revision_id) OR r.episode_revision_id=ANY($3::uuid[]))",
        ),
        (
            "journal",
            "SELECT o.journal_id AS object_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,o.purge_state,r.journal_revision_id AS revision_id,r.title,r.narrative AS text,r.temporal_scope_kind AS time_kind,r.temporal_scope_start AS time_start,r.temporal_scope_end AS time_end,r.formed_at,r.recorded_at FROM journal_objects o JOIN journal_revisions r USING(journal_id) WHERE o.subject_id=$1 AND ((o.journal_id=ANY($2::uuid[]) AND r.journal_revision_id=o.current_revision_id) OR r.journal_revision_id=ANY($3::uuid[]))",
        ),
    ] {
        let object_ids = objects.remove(kind).unwrap_or_default();
        let revision_ids = revisions.remove(kind).unwrap_or_default();
        if object_ids.is_empty() && revision_ids.is_empty() {
            continue;
        }
        let rows = sqlx::query(query)
            .bind(subject.0)
            .bind(&object_ids)
            .bind(&revision_ids)
            .fetch_all(service.store.pool())
            .await
            .map_err(db)?;
        let ids: Vec<Uuid> = rows.iter().map(|row| row.get("revision_id")).collect();
        let evidence = longitudinal_evidence(service, kind, &ids).await?;
        let mut renderings = longitudinal_renderings(
            service,
            subject,
            kind,
            &ids,
            bound
                .config_snapshot
                .get(super::longitudinal_policy::EPISODE_SYNOPSIS)?,
            bound.historical_authority.as_deref(),
        )
        .await?;
        let mut metadata = longitudinal_metadata(
            service,
            subject,
            kind,
            &ids,
            bound.historical_authority.as_deref(),
        )
        .await?;
        for row in rows {
            let revision: Uuid = row.get("revision_id");
            let object: Uuid = row.get("object_id");
            let exact = if kind == "episode" {
                CognitiveRef::EpisodeRevision(EpisodeRevisionId(revision))
            } else {
                CognitiveRef::JournalRevision(JournalRevisionId(revision))
            };
            let mutable = if kind == "episode" {
                CognitiveRef::Episode(EpisodeId(object))
            } else {
                CognitiveRef::Journal(JournalId(object))
            };
            let requested = if references.contains(&exact) {
                &exact
            } else {
                &mutable
            };
            let Some(epoch) = longitudinal_availability(bound, &row, &exact, requested)? else {
                *drops.entry("longitudinal_unavailable".into()).or_default() += 1;
                continue;
            };
            let time = temporal_from_columns(
                row.get("time_kind"),
                row.get("time_start"),
                row.get("time_end"),
            )?;
            let metadata = metadata.remove(&revision).unwrap_or_default();
            if !longitudinal_filter(&bound.source_query, &row, &time, kind, &metadata) {
                *drops.entry("longitudinal_constraints".into()).or_default() += 1;
                continue;
            }
            let basis = if bound.source_query.result_need.need_evidence {
                evidence.get(&revision).cloned().unwrap_or_default()
            } else {
                vec![]
            };
            let mut hit = longitudinal_hit(
                &row,
                kind,
                exact,
                basis,
                renderings.remove(&revision).unwrap_or_default(),
                metadata,
            );
            hit.authority_epoch = Some(epoch);
            hits.push(hit);
        }
    }
    Ok((hits, drops))
}
fn longitudinal_availability(
    bound: &BoundQuery,
    row: &sqlx::postgres::PgRow,
    exact: &CognitiveRef,
    requested: &CognitiveRef,
) -> Result<Option<i64>> {
    let revision: Uuid = row.get("revision_id");
    let header = match super::historical::historical_header(bound, exact) {
        Ok(header) => header,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let epoch = super::historical::header_epoch(header, row)?;
    let historical = header.is_some()
        || (bound.revision_policy.allows_historical(requested)
            && row.get::<Uuid, _>("current_revision_id") != revision);
    let changed_binding = bound.exact_bindings.iter().any(|binding| {
        binding.bound_ref == *exact
            && binding.mutable_object
            && (binding.bound_object_epoch != Some(epoch)
                || (header.is_none() && row.get::<Uuid, _>("current_revision_id") != revision))
    });
    let lifecycle = super::historical::header_text(header, row, "acceptance_state")? == "accepted"
        && super::historical::header_text(header, row, "integrity_state")? == "valid"
        && (super::historical::header_text(header, row, "suppression_state")? == "normal"
            || bound.source_query.expression.constraints.include_suppressed);
    if changed_binding
        || row.get::<String, _>("purge_state") != "normal"
        || (header.is_some() && !lifecycle)
        || (!historical
            && (row.get::<Uuid, _>("current_revision_id") != revision
                || row.get::<String, _>("acceptance_state") != "accepted"
                || row.get::<String, _>("integrity_state") != "valid"
                || (row.get::<String, _>("suppression_state") != "normal"
                    && !bound.source_query.expression.constraints.include_suppressed)))
    {
        return Ok(None);
    }
    Ok(Some(epoch))
}

fn longitudinal_hit(
    row: &sqlx::postgres::PgRow,
    kind: &str,
    exact: CognitiveRef,
    mut evidence: Vec<EvidenceHandle>,
    rendering: LongitudinalRendering,
    metadata: LongitudinalMetadata,
) -> CognitiveHit {
    if !evidence.is_empty() {
        for reference in rendering.sources {
            if !evidence.iter().any(|basis| basis.reference == reference) {
                evidence.push(EvidenceHandle {
                    epistemic_relation: None,
                    reference,
                    basis_role: "interpretation".into(),
                });
            }
        }
    }
    let mut text = format!(
        "{}\n{}\n{}",
        row.get::<Option<String>, _>("title").unwrap_or_default(),
        row.get::<String, _>("text"),
        rendering.text
    );
    if text.len() > 65536 {
        let mut end = 65536;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    CognitiveHit {
        authority_epoch: Some(row.get("object_epoch")),
        preference_refs: vec![],
        reference: exact.clone(),
        revision: Some(exact),
        semantic_role: Some(kind.into()),
        cognitive_role: None,
        formation_mode: None,
        representation: Some(text),
        authority: AuthorityClass::SubjectCognition,
        freshness: FreshnessDescriptor {
            occurred: metadata.occurred,
            observed_at: metadata.observed_at.or_else(|| {
                row.get::<Option<chrono::DateTime<Utc>>, _>("time_end")
                    .or_else(|| row.get("time_start"))
            }),
            valid_time: TemporalExtent::Unknown,
            formed_at: Some(row.get("formed_at")),
            recorded_at: Some(row.get("recorded_at")),
        },
        entity_refs: metadata.entities,
        evidence,
        match_evidence: MatchEvidence::default(),
        materialization: vec![],
    }
}

fn longitudinal_filter(
    query: &CognitiveQuery,
    row: &sqlx::postgres::PgRow,
    time: &TemporalExtent,
    kind: &str,
    metadata: &LongitudinalMetadata,
) -> bool {
    let constraints = &query.expression.constraints;
    constraints.matches_common(QueryFacts {
        authority: AuthorityClass::SubjectCognition,
        entities: &metadata.entities,
        source_classes: &metadata.source_classes,
        modality: Modality::Text,
        cognitive_role: None,
        formation_mode: None,
        epistemic_class: Some(if kind == "journal" {
            EpistemicClass::Narrative
        } else {
            EpistemicClass::Derived
        }),
    }) && constraints.valid.is_none()
        && constraints.occurred.is_none_or(|interval| {
            metadata
                .occurred
                .iter()
                .any(|extent| interval.matches_extent(extent))
        })
        && constraints
            .observed
            .is_none_or(|interval| interval.matches_extent(time))
        && constraints
            .formed
            .is_none_or(|interval| interval.contains(row.get("formed_at")))
        && constraints
            .recorded
            .is_none_or(|interval| interval.contains(row.get("recorded_at")))
}
async fn longitudinal_evidence(
    service: &MemoryService,
    kind: &str,
    ids: &[Uuid],
) -> Result<BTreeMap<Uuid, Vec<EvidenceHandle>>> {
    let mut result = BTreeMap::<Uuid, Vec<EvidenceHandle>>::new();
    if kind == "episode" {
        let rows=sqlx::query("SELECT episode_revision_id,basis_kind,basis_ref,basis_role,occurrence_id,source_region_id,derived_representation_id,derived_region_id,epistemic_relation FROM episode_revision_basis WHERE episode_revision_id=ANY($1::uuid[]) ORDER BY episode_revision_id,basis_no")
            .bind(ids).fetch_all(service.store.pool()).await.map_err(db)?;
        for row in rows {
            let reference = if row.get::<String, _>("basis_kind") == "evidence" {
                if let Some(id) = row.get::<Option<Uuid>, _>("derived_region_id") {
                    CognitiveRef::DerivedRegion(DerivedRegionId(id))
                } else if let Some(id) = row.get::<Option<Uuid>, _>("derived_representation_id") {
                    CognitiveRef::DerivedRepresentation(DerivedRepresentationId(id))
                } else if let Some(id) = row.get::<Option<Uuid>, _>("source_region_id") {
                    CognitiveRef::SourceRegion(SourceRegionId(id))
                } else {
                    CognitiveRef::Occurrence(OccurrenceId(row.get("occurrence_id")))
                }
            } else {
                parse_reference(
                    &row.get::<String, _>("basis_kind"),
                    &row.get::<String, _>("basis_ref"),
                )?
            };
            result
                .entry(row.get("episode_revision_id"))
                .or_default()
                .push(EvidenceHandle {
                    epistemic_relation: nous_core::parse_epistemic_relation(
                        row.get("epistemic_relation"),
                    )?,
                    reference,
                    basis_role: row.get("basis_role"),
                });
        }
    } else {
        let rows=sqlx::query("SELECT journal_revision_id,basis FROM journal_point_basis WHERE journal_revision_id=ANY($1::uuid[]) ORDER BY journal_revision_id,ordinal,basis_no")
            .bind(ids).fetch_all(service.store.pool()).await.map_err(db)?;
        let mut seen = BTreeSet::new();
        for row in rows {
            let revision: Uuid = row.get("journal_revision_id");
            let basis: RevisionBasis = serde_json::from_value(row.get("basis"))
                .map_err(|error| Error::Infrastructure(error.to_string()))?;
            if !seen.insert((revision, basis.canonical_key())) {
                continue;
            }
            let judgment = basis.epistemic_relation();
            let (reference, role) = match basis {
                RevisionBasis::Evidence(evidence) => {
                    (evidence.cognitive_ref(), evidence.basis_role)
                }
                RevisionBasis::CognitionDependency(dependency) => {
                    (dependency.target_revision, dependency.basis_role)
                }
                RevisionBasis::Seed(_) => {
                    return Err(Error::Infrastructure(
                        "Journal has invalid Seed support".into(),
                    ));
                }
            };
            result.entry(revision).or_default().push(EvidenceHandle {
                epistemic_relation: judgment,
                reference,
                basis_role: role.as_str().into(),
            });
        }
    }
    Ok(result)
}

#[derive(Default)]
struct LongitudinalRendering {
    text: String,
    sources: Vec<CognitiveRef>,
}

async fn longitudinal_renderings(
    service: &MemoryService,
    subject: SubjectId,
    kind: &str,
    ids: &[Uuid],
    budget: nous_persistence::EpisodeTextBudget,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, LongitudinalRendering>> {
    let mut result = BTreeMap::<Uuid, LongitudinalRendering>::new();
    if kind == "journal" {
        let rows = sqlx::query("SELECT journal_revision_id,text FROM journal_revision_points WHERE journal_revision_id=ANY($1::uuid[]) ORDER BY journal_revision_id,ordinal")
            .bind(ids).fetch_all(service.store.pool()).await.map_err(db)?;
        for row in rows {
            append_rendering(
                &mut result
                    .entry(row.get("journal_revision_id"))
                    .or_default()
                    .text,
                &row.get::<String, _>("text"),
                budget.total_max_bytes,
            );
        }
    } else {
        let fragments = service
            .store
            .episode_member_text_input_in_view(subject, ids, budget, view)
            .await?;
        for (revision, members) in fragments {
            for member in members {
                if matches!(
                    member.reference,
                    CognitiveRef::DerivedRepresentation(_) | CognitiveRef::DerivedRegion(_)
                ) {
                    result
                        .entry(revision)
                        .or_default()
                        .sources
                        .push(member.reference.clone());
                }
                let text = service
                    .material
                    .text_excerpt(
                        subject,
                        &member.reference,
                        budget.fragment_max_bytes as u64,
                        view,
                    )
                    .await?
                    .map(|view| view.text);
                if let Some(text) = text {
                    append_rendering(
                        &mut result.entry(revision).or_default().text,
                        &text,
                        budget.total_max_bytes,
                    );
                }
            }
        }
    }
    Ok(result)
}

fn append_rendering(output: &mut String, text: &str, maximum: usize) {
    let remaining = maximum.saturating_sub(output.len() + 1);
    if remaining == 0 {
        return;
    }
    let mut end = text.len().min(remaining);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push('\n');
    output.push_str(&text[..end]);
}

#[derive(Default)]
pub(super) struct LongitudinalMetadata {
    entities: Vec<EntityRef>,
    pub(super) source_classes: Vec<SourceClass>,
    pub(super) occurred: Vec<TemporalExtent>,
    pub(super) observed_at: Option<chrono::DateTime<Utc>>,
}

pub(super) async fn longitudinal_metadata(
    service: &MemoryService,
    subject: SubjectId,
    kind: &str,
    ids: &[Uuid],
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, LongitudinalMetadata>> {
    let roots = ids
        .iter()
        .map(|id| parse_reference(&format!("{kind}_revision"), &id.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let mut sql = sqlx::QueryBuilder::new("");
    super::source_facts::push_source_facts(&mut sql, subject, &roots, view)?;
    sql.push("SELECT * FROM source_occurrences ORDER BY root,occurrence_id");
    let rows = sql
        .build()
        .fetch_all(service.store.pool())
        .await
        .map_err(db)?;
    let mut result = BTreeMap::<Uuid, LongitudinalMetadata>::new();
    for row in rows {
        let value = result.entry(row.get("root")).or_default();
        let source = SourceClass::from(row.get::<String, _>("source_class"));
        if !value.source_classes.contains(&source) {
            value.source_classes.push(source);
        }
        let mut entities = row.get::<Vec<String>, _>("entities");
        entities.extend(row.get::<Option<String>, _>("actor_entity_ref"));
        for entity in entities {
            let entity = EntityRef::new(entity)?;
            if !value.entities.contains(&entity) {
                value.entities.push(entity);
            }
        }
        let occurred = temporal_from_columns(
            row.get("occurred_time_kind"),
            row.get("occurred_time_start"),
            row.get("occurred_time_end"),
        )?;
        if !value.occurred.contains(&occurred) {
            value.occurred.push(occurred);
        }
        let observed = row.get::<chrono::DateTime<Utc>, _>("observed_at");
        value.observed_at = Some(
            value
                .observed_at
                .map_or(observed, |current| current.max(observed)),
        );
    }
    Ok(result)
}
