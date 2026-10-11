//! Filter immutable historical headers and source facts before materializing any body.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_persistence::{TimeColumns, push_time_predicate};
use sqlx::{Postgres, QueryBuilder};

pub(super) async fn select(
    service: &MemoryService,
    bound: &BoundQuery,
    family: EvidenceFamily,
    limit: usize,
) -> Result<Vec<CognitiveRef>> {
    let view = bound
        .historical_authority
        .as_deref()
        .ok_or_else(|| Error::Invalid("historical view required".into()))?;
    let references: Vec<_> = view
        .cognition
        .iter()
        .flat_map(|state| state.revisions.iter().cloned())
        .collect();
    let constraints = &bound.source_query.expression.constraints;
    if family == EvidenceFamily::Temporal
        && [
            constraints.occurred,
            constraints.observed,
            constraints.valid,
            constraints.formed,
            constraints.recorded,
        ]
        .iter()
        .all(Option::is_none)
    {
        return Ok(vec![]);
    }
    let mut sql = QueryBuilder::<Postgres>::new("");
    super::super::source_facts::push_source_facts(&mut sql, view.subject, &references, Some(view))?;
    sql.push(r#", catalog AS (
 SELECT s.root_kind kind,r.memory_revision_id id,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,
  ARRAY(SELECT entity_ref FROM memory_revision_aboutness WHERE memory_revision_id=r.memory_revision_id) entities,
  'instant'::text observed_kind,(SELECT max(oc.observed_at) FROM source_occurrences oc JOIN memory_revision_evidence e ON e.memory_revision_id=r.memory_revision_id AND e.occurrence_id=oc.occurrence_id WHERE oc.root_kind=s.root_kind AND oc.root=s.root) observed_start,NULL::timestamptz observed_end
 FROM source_roots s JOIN memory_revisions r ON s.root_kind='memory_revision' AND r.memory_revision_id=s.root JOIN memory_objects o USING(memory_id) WHERE o.purge_state='normal'
 UNION ALL SELECT s.root_kind,r.schema_revision_id,r.valid_time_kind,r.valid_time_start,r.valid_time_end,r.formed_at,r.recorded_at,r.aboutness,
  'instant',(SELECT max(oc.observed_at) FROM source_occurrences oc WHERE oc.root_kind=s.root_kind AND oc.root=s.root),NULL
 FROM source_roots s JOIN cognitive_schema_revisions r ON s.root_kind='cognitive_schema_revision' AND r.schema_revision_id=s.root JOIN cognitive_schemas o USING(schema_id) WHERE o.purge_state='normal'
 UNION ALL SELECT s.root_kind,r.episode_revision_id,'unknown',NULL,NULL,r.formed_at,r.recorded_at,
  ARRAY(SELECT DISTINCT unnest(array_append(oc.entities,oc.actor_entity_ref)) FROM source_occurrences oc WHERE oc.root_kind=s.root_kind AND oc.root=s.root),
  r.experience_time_kind,r.experience_time_start,r.experience_time_end
 FROM source_roots s JOIN episode_revisions r ON s.root_kind='episode_revision' AND r.episode_revision_id=s.root JOIN episode_objects o USING(episode_id) WHERE o.purge_state='normal'
 UNION ALL SELECT s.root_kind,r.journal_revision_id,'unknown',NULL,NULL,r.formed_at,r.recorded_at,
  ARRAY(SELECT DISTINCT unnest(array_append(oc.entities,oc.actor_entity_ref)) FROM source_occurrences oc WHERE oc.root_kind=s.root_kind AND oc.root=s.root),
  r.temporal_scope_kind,r.temporal_scope_start,r.temporal_scope_end
 FROM source_roots s JOIN journal_revisions r ON s.root_kind='journal_revision' AND r.journal_revision_id=s.root JOIN journal_objects o USING(journal_id) WHERE o.purge_state='normal'
) SELECT c.kind,c.id FROM catalog c WHERE true
"#);
    match family {
        EvidenceFamily::Entity => {
            let entities = entities(&bound.source_query);
            if entities.is_empty() {
                return Ok(vec![]);
            }
            sql.push(" AND c.entities && ")
                .push_bind(entities)
                .push("::text[]");
        }
        EvidenceFamily::SchemaDirect => {
            let roots = schema_roots(bound, view);
            if roots.is_empty() {
                return Ok(vec![]);
            }
            sql.push(" AND ((c.kind='cognitive_schema_revision' AND c.id=ANY(").push_bind(roots.clone()).push("::uuid[])) OR EXISTS(SELECT 1 FROM cognitive_schema_evidence_links l WHERE l.schema_revision_id=ANY(")
                .push_bind(roots).push("::uuid[]) AND l.link_id=ANY(").push_bind(view.schema_evidence_links.clone()).push("::uuid[]) AND l.role='support' AND l.epistemic_relation NOT IN ('contradicts','weakens','corrects','counterexample') AND l.basis_kind=c.kind AND l.basis_ref=c.id::text))");
        }
        EvidenceFamily::Temporal => (),
        _ => return Err(Error::Invalid("not an owner-direct family".into())),
    }
    for (columns, predicate) in [
        (
            TimeColumns::Extent {
                kind: "c.valid_time_kind",
                start: "c.valid_time_start",
                end: "c.valid_time_end",
            },
            constraints.valid,
        ),
        (
            TimeColumns::Extent {
                kind: "c.observed_kind",
                start: "c.observed_start",
                end: "c.observed_end",
            },
            constraints.observed,
        ),
        (TimeColumns::Instant("c.formed_at"), constraints.formed),
        (TimeColumns::Instant("c.recorded_at"), constraints.recorded),
    ] {
        push_time_predicate_if(&mut sql, columns, predicate)?;
    }
    if let Some(predicate) = constraints.occurred {
        sql.push(" AND EXISTS(SELECT 1 FROM source_occurrences oc WHERE oc.root_kind=c.kind AND oc.root=c.id AND (c.kind<>'memory_revision' OR EXISTS(SELECT 1 FROM memory_revision_evidence e WHERE e.memory_revision_id=c.id AND e.occurrence_id=oc.occurrence_id))");
        push_time_predicate(
            &mut sql,
            TimeColumns::Extent {
                kind: "oc.occurred_time_kind",
                start: "oc.occurred_time_start",
                end: "oc.occurred_time_end",
            },
            predicate,
        )?;
        sql.push(")");
    }
    if family == EvidenceFamily::SchemaDirect {
        sql.push(" ORDER BY c.kind COLLATE \"C\",c.id");
    } else {
        sql.push(" ORDER BY c.recorded_at DESC,c.kind COLLATE \"C\",c.id");
    }
    sql.push(" LIMIT ").push_bind(limit as i64);
    let rows = sql
        .build()
        .fetch_all(service.store.pool())
        .await
        .map_err(db)?;
    rows.into_iter()
        .map(|row| {
            parse_reference(
                &row.get::<String, _>("kind"),
                &row.get::<Uuid, _>("id").to_string(),
            )
        })
        .collect()
}

fn push_time_predicate_if(
    sql: &mut QueryBuilder<Postgres>,
    columns: TimeColumns,
    predicate: Option<TimePredicate>,
) -> Result<()> {
    if let Some(predicate) = predicate {
        push_time_predicate(sql, columns, predicate)?;
    }
    Ok(())
}

fn entities(query: &CognitiveQuery) -> Vec<String> {
    query
        .expression
        .cues
        .iter()
        .filter_map(|cue| match cue {
            Cue::Entity(cue) => Some(cue.entity_ref.as_str().to_owned()),
            _ => None,
        })
        .chain(
            query
                .expression
                .constraints
                .entity_requirements
                .iter()
                .map(|entity| entity.as_str().to_owned()),
        )
        .chain(
            query
                .expression
                .targets
                .iter()
                .filter_map(|target| match target {
                    QueryTarget::EntityNeighborhood { entity_ref } => {
                        Some(entity_ref.as_str().to_owned())
                    }
                    _ => None,
                }),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn schema_roots(bound: &BoundQuery, view: &HistoricalAuthoritySnapshot) -> Vec<Uuid> {
    let objects: HashSet<_> = bound
        .source_query
        .expression
        .cues
        .iter()
        .filter_map(|cue| match cue {
            Cue::Schema(cue) => Some(CognitiveRef::CognitiveSchema(cue.schema)),
            _ => None,
        })
        .chain(
            bound
                .source_query
                .expression
                .targets
                .iter()
                .filter_map(|target| match target {
                    QueryTarget::SchemaNeighborhood { schema } => {
                        Some(CognitiveRef::CognitiveSchema(*schema))
                    }
                    _ => None,
                }),
        )
        .collect();
    view.cognition
        .iter()
        .filter(|state| objects.contains(&state.object))
        .flat_map(|state| state.revisions.iter())
        .chain(
            bound
                .exact_bindings
                .iter()
                .map(|binding| &binding.bound_ref),
        )
        .filter_map(|reference| match reference {
            CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0),
            _ => None,
        })
        .collect()
}
