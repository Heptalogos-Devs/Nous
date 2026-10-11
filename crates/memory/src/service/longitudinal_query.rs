// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::BoundQuery;
use std::collections::{BTreeMap, BTreeSet, HashMap};

struct LongitudinalCandidate<'a> {
    requested: CognitiveRef,
    view: &'a CognitionView,
}

pub(super) async fn materialize_longitudinal(
    service: &MemoryService,
    bound: &BoundQuery,
    references: &[CognitiveRef],
    views: &HashMap<CognitiveRef, CognitionView>,
) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)> {
    let subject = bound.source_query.subject;
    let mut selected = HashMap::<CognitiveRef, LongitudinalCandidate<'_>>::new();
    let mut selected_refs = Vec::new();
    let mut drops = BTreeMap::new();
    for reference in references {
        if !matches!(
            reference,
            CognitiveRef::Episode(_)
                | CognitiveRef::EpisodeRevision(_)
                | CognitiveRef::Journal(_)
                | CognitiveRef::JournalRevision(_)
        ) {
            continue;
        }
        let Some(view) = views.get(reference) else {
            if bound.read_context.is_historical()
                && bound
                    .read_context
                    .historical_view()
                    .is_some_and(|snapshot| {
                        snapshot.cognition_for(reference).is_none()
                            || (matches!(
                                reference,
                                CognitiveRef::EpisodeRevision(_) | CognitiveRef::JournalRevision(_)
                            ) && snapshot.selected_cognition_for(reference).is_none())
                    })
            {
                *drops.entry("longitudinal_unavailable".into()).or_default() += 1;
            }
            continue;
        };
        if !matches!(view, CognitionView::Episode(_) | CognitionView::Journal(_)) {
            continue;
        }
        let exact = view.revision_header().revision_ref.clone();
        let requested = if references.contains(&exact) {
            exact.clone()
        } else {
            reference.clone()
        };
        match selected.entry(exact.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                selected_refs.push(exact.clone());
                entry.insert(LongitudinalCandidate { requested, view });
            }
            std::collections::hash_map::Entry::Occupied(mut entry)
                if references.contains(&exact) =>
            {
                entry.insert(LongitudinalCandidate {
                    requested: exact,
                    view,
                });
            }
            std::collections::hash_map::Entry::Occupied(_) => {}
        }
    }
    let mut hits = Vec::new();
    for kind in ["episode", "journal"] {
        let family = selected_refs
            .iter()
            .filter_map(|reference| {
                let belongs = match reference {
                    CognitiveRef::EpisodeRevision(_) => kind == "episode",
                    CognitiveRef::JournalRevision(_) => kind == "journal",
                    _ => false,
                };
                belongs
                    .then(|| {
                        selected
                            .get(reference)
                            .map(|candidate| (reference.clone(), candidate))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        if family.is_empty() {
            continue;
        }
        let revision_ids = family
            .iter()
            .filter_map(|(reference, _)| match reference {
                CognitiveRef::EpisodeRevision(id) if kind == "episode" => Some(id.0),
                CognitiveRef::JournalRevision(id) if kind == "journal" => Some(id.0),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut metadata = super::longitudinal_query::longitudinal_metadata(
            service,
            subject,
            kind,
            &revision_ids,
            bound.read_context.historical_view(),
        )
        .await?;
        let mut renderings = longitudinal_renderings(
            service,
            subject,
            kind,
            &family,
            &revision_ids,
            bound
                .read_context
                .config_snapshot
                .get(super::longitudinal_policy::EPISODE_SYNOPSIS)?,
            bound.read_context.historical_view(),
        )
        .await?;

        for (exact, candidate) in &family {
            let Some(epoch) =
                longitudinal_availability(bound, candidate.view, exact, &candidate.requested)?
            else {
                *drops.entry("longitudinal_unavailable".into()).or_default() += 1;
                continue;
            };
            let (time, formed_at, recorded_at) = longitudinal_temporal_fields(candidate.view);
            let revision = candidate.view.revision_header().revision_id();
            let metadata = metadata.remove(&revision).unwrap_or_default();
            if !longitudinal_filter(
                &bound.source_query,
                &time,
                kind,
                &metadata,
                formed_at,
                recorded_at,
            ) {
                *drops.entry("longitudinal_constraints".into()).or_default() += 1;
                continue;
            }
            let basis = if bound.source_query.result_need.need_evidence {
                longitudinal_evidence(candidate.view, kind)?
            } else {
                vec![]
            };
            let mut hit = longitudinal_hit(
                candidate.view,
                kind,
                exact.clone(),
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
    view: &CognitionView,
    exact: &CognitiveRef,
    requested: &CognitiveRef,
) -> Result<Option<i64>> {
    let header = view.header();
    let revision = view.revision_header();
    let frozen_historical = bound.read_context.is_historical();
    let historical = frozen_historical
        || (bound.revision_policy.allows_historical(requested)
            && header.current_revision_id() != revision.revision_id());
    let changed_binding = bound.exact_bindings.iter().any(|binding| {
        binding.bound_ref == *exact
            && binding.mutable_object
            && (binding.bound_object_epoch != Some(header.object_epoch)
                || (!frozen_historical && header.current_revision_id() != revision.revision_id()))
    });
    let lifecycle = header.lifecycle.acceptance == AcceptanceState::Accepted
        && header.lifecycle.integrity == IntegrityState::Valid
        && (header.lifecycle.suppression == SuppressionState::Normal
            || bound.source_query.expression.constraints.include_suppressed);
    if changed_binding
        || header.lifecycle.purge != PurgeState::Normal
        || (frozen_historical && !lifecycle)
        || (!historical && (header.current_revision_id() != revision.revision_id() || !lifecycle))
    {
        return Ok(None);
    }
    Ok(Some(header.object_epoch))
}

fn longitudinal_hit(
    view: &CognitionView,
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
    let (header, revision, title, text, time) = match view {
        CognitionView::Episode(view) => (
            view.header.clone(),
            view.revision.header.clone(),
            view.revision.title.clone().unwrap_or_default(),
            view.revision.boundary_explanation.clone(),
            view.revision.experience_time.clone(),
        ),
        CognitionView::Journal(view) => (
            view.header.clone(),
            view.revision.header.clone(),
            view.revision.title.clone().unwrap_or_default(),
            view.revision.narrative.clone(),
            view.revision.temporal_scope.clone(),
        ),
        _ => unreachable!("longitudinal materialization needs Episode or Journal"),
    };
    let mut text = format!("{}\n{}\n{}", title, text, rendering.text);
    if text.len() > 65536 {
        let mut end = 65536;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    CognitiveHit {
        authority_epoch: Some(header.object_epoch),
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
            observed_at: metadata.observed_at.or_else(|| temporal_endpoint(&time)),
            valid_time: TemporalExtent::Unknown,
            formed_at: Some(revision.formed_at),
            recorded_at: Some(revision.recorded_at),
        },
        entity_refs: metadata.entities,
        evidence,
        match_evidence: MatchEvidence::default(),
        materialization: vec![],
    }
}

fn longitudinal_filter(
    query: &CognitiveQuery,
    time: &TemporalExtent,
    kind: &str,
    metadata: &LongitudinalMetadata,
    formed_at: chrono::DateTime<Utc>,
    recorded_at: chrono::DateTime<Utc>,
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
            .is_none_or(|interval| interval.contains(formed_at))
        && constraints
            .recorded
            .is_none_or(|interval| interval.contains(recorded_at))
}

fn longitudinal_temporal_fields(
    view: &CognitionView,
) -> (TemporalExtent, chrono::DateTime<Utc>, chrono::DateTime<Utc>) {
    match view {
        CognitionView::Episode(view) => (
            view.revision.experience_time.clone(),
            view.revision.header.formed_at,
            view.revision.header.recorded_at,
        ),
        CognitionView::Journal(view) => (
            view.revision.temporal_scope.clone(),
            view.revision.header.formed_at,
            view.revision.header.recorded_at,
        ),
        _ => unreachable!("longitudinal time belongs to Episode or Journal"),
    }
}

fn temporal_endpoint(extent: &TemporalExtent) -> Option<chrono::DateTime<Utc>> {
    match extent {
        TemporalExtent::Unknown => None,
        TemporalExtent::Instant { at } => Some(*at),
        TemporalExtent::Interval { start, end } => end.or(*start),
    }
}

fn longitudinal_evidence(view: &CognitionView, kind: &str) -> Result<Vec<EvidenceHandle>> {
    let basis = match view {
        CognitionView::Episode(view) => view.basis.clone(),
        CognitionView::Journal(view) => view
            .points
            .iter()
            .flat_map(|point| point.basis.iter().cloned())
            .collect(),
        _ => return Ok(Vec::new()),
    };
    let mut seen = BTreeSet::new();
    let mut evidence = Vec::with_capacity(basis.len());
    for basis in basis {
        if kind == "journal" && !seen.insert(basis.canonical_key()) {
            continue;
        }
        let judgment = basis.epistemic_relation();
        let (reference, role) = match basis {
            RevisionBasis::Evidence(evidence) => (evidence.cognitive_ref(), evidence.basis_role),
            RevisionBasis::CognitionDependency(dependency) => {
                (dependency.target_revision, dependency.basis_role)
            }
            RevisionBasis::Seed(_) => {
                return Err(Error::new(
                    nous_core::ErrorOwner::Memory,
                    nous_core::ErrorCode::InternalFailure,
                    if kind == "journal" {
                        "Journal point basis cannot be a cognitive seed"
                    } else {
                        "Episode basis cannot be a cognitive seed"
                    },
                ));
            }
        };
        evidence.push(EvidenceHandle {
            epistemic_relation: judgment,
            reference,
            basis_role: role.as_str().into(),
        });
    }
    Ok(evidence)
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
    family: &[(CognitiveRef, &LongitudinalCandidate<'_>)],
    ids: &[Uuid],
    budget: nous_persistence::EpisodeTextBudget,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, LongitudinalRendering>> {
    let mut result = BTreeMap::<Uuid, LongitudinalRendering>::new();
    if kind == "journal" {
        for (reference, candidate) in family {
            let CognitiveRef::JournalRevision(revision) = reference else {
                continue;
            };
            let CognitionView::Journal(view) = candidate.view else {
                continue;
            };
            for point in &view.points {
                append_rendering(
                    &mut result.entry(revision.0).or_default().text,
                    &point.text,
                    budget.total_max_bytes,
                );
            }
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
            ErrorOwner::Memory,
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
