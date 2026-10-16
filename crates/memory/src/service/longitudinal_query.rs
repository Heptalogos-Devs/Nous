// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_runtime::BoundQuery;
use std::collections::{BTreeMap, BTreeSet};

#[expect(
    clippy::too_many_lines,
    reason = "captured source authorization and bounded owner materialization stay in one read view"
)]
pub(super) async fn materialize_longitudinal(
    service: &MemoryService,
    bound: &BoundQuery,
    references: &[CognitiveRef],
) -> Result<(Vec<CognitiveHit>, BTreeMap<String, usize>)> {
    let subject = bound.source_query.subject;
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
            "SELECT o.episode_id AS object_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,r.episode_revision_id AS revision_id,r.title,r.boundary_explanation AS text,r.experience_time_kind AS time_kind,r.experience_time_start AS time_start,r.experience_time_end AS time_end,r.formed_at,r.formed_time_domain_ref,r.formed_recorded_at,r.recorded_at FROM episode_objects o JOIN episode_revisions r USING(episode_id) WHERE o.subject_id=$1 AND ((o.episode_id=ANY($2::uuid[]) AND r.episode_revision_id=o.current_revision_id) OR r.episode_revision_id=ANY($3::uuid[]))",
        ),
        (
            "journal",
            "SELECT o.journal_id AS object_id,o.current_revision_id,o.object_epoch,o.acceptance_state,o.integrity_state,o.suppression_state,r.journal_revision_id AS revision_id,r.title,r.narrative AS text,r.temporal_scope_kind AS time_kind,r.temporal_scope_start AS time_start,r.temporal_scope_end AS time_end,r.formed_at,r.formed_time_domain_ref,r.formed_recorded_at,r.recorded_at FROM journal_objects o JOIN journal_revisions r USING(journal_id) WHERE o.subject_id=$1 AND ((o.journal_id=ANY($2::uuid[]) AND r.journal_revision_id=o.current_revision_id) OR r.journal_revision_id=ANY($3::uuid[]))",
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
            .fetch_all(&mut *tx)
            .await
            .map_err(db)?;
        let rows = rows
            .into_iter()
            .filter_map(
                |row| match longitudinal_availability(bound, &row, kind, references) {
                    Ok(Some((exact, epoch))) => Some(Ok((row, exact, epoch))),
                    Ok(None) => {
                        *drops.entry("longitudinal_unavailable".into()).or_default() += 1;
                        None
                    }
                    Err(error) => Some(Err(error)),
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let ids = rows
            .iter()
            .map(|(row, _, _)| row.get("revision_id"))
            .collect::<Vec<Uuid>>();
        let evidence = if bound.source_query.result_need.need_evidence {
            longitudinal_evidence(
                service,
                &mut tx,
                subject,
                kind,
                &ids,
                bound.historical_authority.as_deref(),
            )
            .await?
        } else {
            BTreeMap::new()
        };
        let mut renderings = longitudinal_renderings(
            service,
            &mut tx,
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
            &mut tx,
            subject,
            kind,
            &ids,
            bound.historical_authority.as_deref(),
            bound.source_query.source_read.as_ref(),
        )
        .await?;
        for (row, exact, epoch) in rows {
            let revision: Uuid = row.get("revision_id");
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
            let basis = evidence.get(&revision).cloned().unwrap_or_default();
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
    kind: &str,
    references: &[CognitiveRef],
) -> Result<Option<(CognitiveRef, i64)>> {
    let revision = row.get("revision_id");
    let object = row.get("object_id");
    let (exact, mutable) = if kind == "episode" {
        (
            CognitiveRef::EpisodeRevision(EpisodeRevisionId(revision)),
            CognitiveRef::Episode(EpisodeId(object)),
        )
    } else {
        (
            CognitiveRef::JournalRevision(JournalRevisionId(revision)),
            CognitiveRef::Journal(JournalId(object)),
        )
    };
    let requested = if references.contains(&exact) {
        &exact
    } else {
        &mutable
    };
    let revision: Uuid = row.get("revision_id");
    let header = match super::historical::historical_header(bound, &exact) {
        Ok(header) => header,
        Err(Error::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let epoch = super::historical::header_epoch(header, row)?;
    let historical = header.is_some()
        || (bound.revision_policy.allows_historical(requested)
            && row.get::<Uuid, _>("current_revision_id") != revision);
    let changed_binding = bound.exact_bindings.iter().any(|binding| {
        binding.bound_ref == exact
            && binding.mutable_object
            && (binding.bound_object_epoch != Some(epoch)
                || (header.is_none() && row.get::<Uuid, _>("current_revision_id") != revision))
    });
    let lifecycle = super::historical::header_text(header, row, "acceptance_state")? == "accepted"
        && super::historical::header_text(header, row, "integrity_state")? == "valid"
        && (super::historical::header_text(header, row, "suppression_state")? == "normal"
            || bound.source_query.expression.constraints.include_suppressed);
    if changed_binding
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
    Ok(Some((exact, epoch)))
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
    text.truncate(text.floor_char_boundary(65536));
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
            temporal_evidence: metadata.temporal,
            valid_time: DomainTemporalExtent::default(),
            formed_at: Some(DomainStamp {
                time_domain_ref: row.get("formed_time_domain_ref"),
                at: row.get("formed_at"),
                recorded_at: row.get("formed_recorded_at"),
            }),
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
    _time: &TemporalExtent,
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
        && metadata.temporal.matches(
            query.temporal_predicate(TimeAxis::Occurred).as_ref(),
            query.temporal_predicate(TimeAxis::Observed).as_ref(),
        )
        && query.temporal_predicate(TimeAxis::Formed).is_none_or(|p| {
            p.matches_stamp(&DomainStamp {
                time_domain_ref: row.get("formed_time_domain_ref"),
                at: row.get("formed_at"),
                recorded_at: row.get("formed_recorded_at"),
            }) == Some(true)
        })
        && constraints
            .recorded
            .is_none_or(|p| p.contains(row.get("recorded_at")))
}
async fn longitudinal_evidence(
    service: &MemoryService,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    kind: &str,
    ids: &[Uuid],
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, Vec<EvidenceHandle>>> {
    let references = ids
        .iter()
        .map(|id| {
            if kind == "episode" {
                CognitiveRef::EpisodeRevision(EpisodeRevisionId(*id))
            } else {
                CognitiveRef::JournalRevision(JournalRevisionId(*id))
            }
        })
        .collect::<Vec<_>>();
    let mut result = BTreeMap::new();
    for references in references.chunks(256) {
        for (reference, support) in service
            .exact_supports_in(tx, subject, references, view)
            .await?
        {
            let mut seen = BTreeSet::new();
            let handles = support
                .basis
                .iter()
                .filter(|basis| kind != "journal" || seen.insert(basis.canonical_key()))
                .map(RevisionBasis::evidence_handle)
                .collect();
            result.insert(reference.uuid().expect("longitudinal revision"), handles);
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
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    kind: &str,
    ids: &[Uuid],
    budget: nous_persistence::EpisodeTextBudget,
    view: Option<&HistoricalAuthoritySnapshot>,
) -> Result<BTreeMap<Uuid, LongitudinalRendering>> {
    let mut result = BTreeMap::<Uuid, LongitudinalRendering>::new();
    if kind == "journal" {
        let rows = sqlx::query("SELECT journal_revision_id,text FROM journal_revision_points WHERE journal_revision_id=ANY($1::uuid[]) ORDER BY journal_revision_id,ordinal")
            .bind(ids).fetch_all(&mut **tx).await.map_err(db)?;
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
        let members = fragments
            .into_iter()
            .flat_map(|(revision, members)| {
                members
                    .into_iter()
                    .map(move |member| (revision, member.reference))
            })
            .collect::<Vec<_>>();
        for members in members.chunks(256) {
            let references = members
                .iter()
                .map(|(_, reference)| reference.clone())
                .collect::<Vec<_>>();
            let texts = service
                .material
                .text_excerpts(subject, &references, budget.fragment_max_bytes as u64, view)
                .await?;
            for (revision, reference) in members {
                let rendering = result.entry(*revision).or_default();
                if matches!(
                    reference,
                    CognitiveRef::DerivedRepresentation(_) | CognitiveRef::DerivedRegion(_)
                ) {
                    rendering.sources.push(reference.clone());
                }
                if let Some(text) = texts.get(reference) {
                    append_rendering(&mut rendering.text, &text.text, budget.total_max_bytes);
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
    output.push('\n');
    output.push_str(&text[..text.floor_char_boundary(remaining)]);
}

#[derive(Default)]
pub(super) struct LongitudinalMetadata {
    entities: Vec<EntityRef>,
    pub(super) source_classes: Vec<SourceClass>,
    pub(super) temporal: TemporalEvidence,
}

pub(super) async fn longitudinal_metadata(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    subject: SubjectId,
    kind: &str,
    ids: &[Uuid],
    view: Option<&HistoricalAuthoritySnapshot>,
    read: Option<&SourceReadView>,
) -> Result<BTreeMap<Uuid, LongitudinalMetadata>> {
    let roots = ids
        .iter()
        .map(|id| parse_reference(&format!("{kind}_revision"), &id.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let mut sql = sqlx::QueryBuilder::new("");
    super::source_facts::push_source_facts(&mut sql, subject, &roots, view, read)?;
    sql.push("SELECT * FROM source_occurrences ORDER BY root,occurrence_id");
    let rows = sql.build().fetch_all(&mut **tx).await.map_err(db)?;
    let mut result = BTreeMap::<Uuid, LongitudinalMetadata>::new();
    for row in rows {
        if view.is_some_and(|view| {
            !view.material_contains(&CognitiveRef::Occurrence(OccurrenceId(
                row.get("occurrence_id"),
            )))
        }) {
            continue;
        }
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
        value.temporal.sources.push(SourceTemporalEvidence {
            occurrence_id: OccurrenceId(row.get("occurrence_id")),
            occurred: DomainTemporalExtent {
                time_domain_ref: row.get("occurred_time_domain_ref"),
                extent: temporal_from_columns(
                    row.get("occurred_time_kind"),
                    row.get("occurred_time_start"),
                    row.get("occurred_time_end"),
                )?,
            },
            observed: serde_json::from_value(row.get("observed_stamp"))
                .map_err(|e| Error::Infrastructure(e.to_string()))?,
            observed_at: row.get("observed_at"),
            recorded_at: row.get("source_recorded_at"),
        });
    }
    Ok(result)
}
