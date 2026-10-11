// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;
use sqlx::Row;
use std::collections::BTreeSet;

impl KernelService {
    #[expect(
        clippy::too_many_lines,
        reason = "one bounded snapshot captures source, owner identities, candidates and eligible support catalogs"
    )]
    pub(super) async fn consolidation_context(&self, plan: &mut k::MaintenancePlan) -> Result<()> {
        let subject = SubjectId(id(&plan.subject_id)?);
        let snapshot = self.0.configuration.snapshot_for_subject(subject)?;
        let policy = snapshot.get(nous_memory::CONSOLIDATION_CONTEXT)?;
        let source = from_ref(required(
            required(plan.consolidation_source.clone(), "source")?.reference,
            "source reference",
        )?)?;
        let text = plan
            .journal
            .as_ref()
            .map(|journal| journal.narrative.as_str())
            .or_else(|| plan.members.first().map(|member| member.text.as_str()))
            .unwrap_or("");
        let cue: String = text.chars().take(policy.query_cue_chars).collect();
        let query = consolidation_query(subject, cue, policy.candidate_limit);
        let hits = if text.trim().is_empty() {
            Vec::new()
        } else {
            Box::pin(self.0.query(query)).await?.results
        };
        let schema_ids: Vec<uuid::Uuid> = hits
            .iter()
            .filter_map(
                |hit| match hit.revision.as_ref().unwrap_or(&hit.reference) {
                    CognitiveRef::CognitiveSchemaRevision(id) => Some(id.0),
                    _ => None,
                },
            )
            .collect();
        let schemas: std::collections::BTreeMap<uuid::Uuid, sqlx::postgres::PgRow> = sqlx::query("SELECT schema_revision_id,schema_id,formation_kind,title,applicability_description,boundary_definition,tags FROM cognitive_schema_revisions WHERE schema_revision_id=ANY($1::uuid[])")
            .bind(schema_ids).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)?
            .into_iter().map(|row| (row.get("schema_revision_id"), row)).collect();
        let memory_ids: Vec<uuid::Uuid> = hits
            .iter()
            .filter_map(
                |hit| match hit.revision.as_ref().unwrap_or(&hit.reference) {
                    CognitiveRef::MemoryRevision(id) => Some(id.0),
                    _ => None,
                },
            )
            .collect();
        let memory_objects: std::collections::BTreeMap<uuid::Uuid, uuid::Uuid> = sqlx::query("SELECT memory_revision_id,memory_id FROM memory_revisions WHERE subject_id=$1 AND memory_revision_id=ANY($2::uuid[])").bind(subject.0).bind(memory_ids).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)?.into_iter().map(|row| (row.get("memory_revision_id"),row.get("memory_id"))).collect();
        let references: Vec<_> = hits
            .iter()
            .map(|hit| hit.revision.as_ref().unwrap_or(&hit.reference).clone())
            .collect();
        let mut use_summaries = self
            .consolidation_use_summaries(subject, &references)
            .await?;
        let mut scopes = vec![source];
        let mut entities = BTreeSet::new();
        entities.extend(
            self.consolidation_named_entities(plan, subject, policy.entity_limit)
                .await?,
        );
        for member in &plan.members {
            if let Some(entity) = &member.actor_entity_ref {
                entities.insert(entity.clone());
            }
        }
        for hit in hits {
            let reference = hit.revision.unwrap_or(hit.reference);
            if !matches!(
                reference,
                CognitiveRef::MemoryRevision(_) | CognitiveRef::CognitiveSchemaRevision(_)
            ) {
                continue;
            }
            let Some(epoch) = hit.authority_epoch else {
                continue;
            };
            entities.extend(
                hit.entity_refs
                    .iter()
                    .map(|entity| entity.as_str().to_owned()),
            );
            scopes.push(reference.clone());
            let mut text = hit.representation.unwrap_or_default();
            let formation = if let CognitiveRef::CognitiveSchemaRevision(id) = &reference {
                if let Some(schema) = schemas.get(&id.0) {
                    text = format!(
                        "Title: {}\nClaim: {}\nApplicability: {}\nBoundary: {}\nTags: {:?}",
                        schema.get::<Option<String>, _>("title").unwrap_or_default(),
                        text,
                        schema.get::<String, _>("applicability_description"),
                        schema.get::<String, _>("boundary_definition"),
                        schema.get::<Vec<uuid::Uuid>, _>("tags")
                    );
                    schema.get::<String, _>("formation_kind")
                } else {
                    String::new()
                }
            } else {
                hit.formation_mode.unwrap_or_default()
            };
            let text_truncated = text.chars().count() > policy.candidate_text_chars;
            let text: String = text.chars().take(policy.candidate_text_chars).collect();
            let uses = use_summaries
                .remove(&reference.to_string())
                .unwrap_or_default();
            plan.candidates.push(k::ConsolidationCandidate {
                key: reference.to_string(),
                target: Some(k::ExpectedCognition {
                    object_id: match &reference {
                        CognitiveRef::MemoryRevision(id) => memory_objects
                            .get(&id.0)
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        CognitiveRef::CognitiveSchemaRevision(id) => schemas
                            .get(&id.0)
                            .map(|row| row.get::<uuid::Uuid, _>("schema_id").to_string())
                            .unwrap_or_default(),
                        _ => unreachable!(),
                    },
                    reference: Some(to_ref(reference)),
                    expected_epoch: epoch,
                }),
                eligible_basis_keys: Vec::new(),
                text,
                text_truncated,
                cognitive_role: hit.cognitive_role.unwrap_or_default(),
                formation_mode: formation,
                entity_refs: hit
                    .entity_refs
                    .into_iter()
                    .map(|entity| entity.as_str().to_owned())
                    .collect(),
                r#use: uses,
                valid_time: Some(temporal_proto(&hit.freshness.valid_time)),
                formed_at: hit.freshness.formed_at.map(timestamp),
                recorded_at: hit.freshness.recorded_at.map(timestamp),
            });
        }
        plan.max_consolidation_actions =
            snapshot.get(nous_memory::CONSOLIDATION_MAX_ACTIONS)? as u32;
        self.finish_consolidation_catalog(plan, subject, &scopes, entities, &policy)
            .await
    }

    async fn consolidation_named_entities(
        &self,
        plan: &k::MaintenancePlan,
        subject: SubjectId,
        limit: usize,
    ) -> Result<Vec<String>> {
        // Directory candidates are selectors, not asserted aboutness. The model must select them.
        let cue = plan
            .journal
            .as_ref()
            .map(|journal| journal.narrative.clone())
            .unwrap_or_else(|| {
                plan.members
                    .iter()
                    .map(|member| member.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        sqlx::query_scalar("SELECT DISTINCT b.canonical_ref FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE v.subject_id=$1 AND b.object_kind='entity' AND b.tombstoned_at IS NULL AND ((length(v.display_name)>1 AND strpos(lower($2),lower(v.display_name))>0) OR EXISTS(SELECT 1 FROM unnest(v.aliases) alias WHERE length(alias)>1 AND strpos(lower($2),lower(alias))>0)) ORDER BY b.canonical_ref LIMIT $3")
            .bind(subject.0).bind(cue).bind(i64::try_from(limit).map_err(|_|Error::Invalid("entity catalog limit exceeded".into()))?).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)
    }
    async fn finish_consolidation_catalog(
        &self,
        plan: &mut k::MaintenancePlan,
        subject: SubjectId,
        scopes: &[CognitiveRef],
        entities: BTreeSet<String>,
        policy: &nous_memory::ConsolidationContextPolicy,
    ) -> Result<()> {
        let memory = self.require_memory()?;
        let basis = memory.consolidation_catalog_basis(subject, scopes).await?;
        let provenance = memory.provenance_summary(subject, &basis).await?;
        plan.basis_catalog_partial = basis.len() > policy.basis_limit;
        plan.provenance_roots_partial = provenance.roots.len() > policy.provenance_root_limit;
        plan.provenance_roots = provenance
            .roots
            .into_iter()
            .take(policy.provenance_root_limit)
            .map(|root| k::ProvenanceRoot {
                key: root.root_key,
                certainty: enum_name(root.certainty),
            })
            .collect();
        let source_support = RevisionBasis::CognitionDependency(CognitionDependency {
            epistemic_relation: None,
            target_revision: scopes[0].clone(),
            basis_role: BasisRole::Direct,
        });
        let source_key = source_support.canonical_key();
        let bounded = std::iter::once(source_support)
            .chain(
                basis
                    .into_iter()
                    .filter(|basis| basis.canonical_key() != source_key),
            )
            .take(policy.basis_limit);
        plan.basis = bounded
            .map(|basis| k::BasisCatalogEntry {
                key: basis.canonical_key(),
                basis: Some(basis_proto(basis)),
            })
            .collect();
        for candidate in &mut plan.candidates {
            let target = required(candidate.target.as_ref(), "target")?;
            let reference = from_ref(required(target.reference.clone(), "reference")?)?;
            let query = match reference {
                CognitiveRef::MemoryRevision(_) => {
                    "SELECT memory_revision_id FROM memory_revisions WHERE subject_id=$1 AND memory_id=$2"
                }
                CognitiveRef::CognitiveSchemaRevision(_) => {
                    "SELECT r.schema_revision_id FROM cognitive_schema_revisions r JOIN cognitive_schemas s USING(schema_id) WHERE s.subject_id=$1 AND s.schema_id=$2"
                }
                _ => unreachable!(),
            };
            let revisions: Vec<uuid::Uuid> = sqlx::query_scalar(query)
                .bind(subject.0)
                .bind(id(&target.object_id)?)
                .fetch_all(self.0.store.pool())
                .await
                .map_err(nous_persistence::database_error)?;
            candidate.eligible_basis_keys = plan.basis.iter().filter(|entry| !matches!(entry.basis.as_ref().and_then(|basis| basis.basis.as_ref()), Some(p::revision_basis::Basis::CognitionDependency(dependency)) if dependency.target_revision.as_ref().is_some_and(|target| target.kind == reference_parts(&reference).0 && revisions.iter().any(|id| id.to_string() == target.value)))).map(|entry| entry.key.clone()).collect();
        }
        plan.entities = entities
            .into_iter()
            .take(policy.entity_limit)
            .enumerate()
            .map(|(index, entity_ref)| k::ConsolidationEntity {
                key: format!("entity{index}"),
                entity_ref,
            })
            .collect();
        Ok(())
    }
    async fn consolidation_use_summaries(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<std::collections::BTreeMap<String, Vec<k::UseSummary>>> {
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query("SELECT e.ref_kind,e.ref_value,e.use_kind,COUNT(*) AS count,MAX(e.occurred_at) AS last_used FROM cognitive_use_events e JOIN unnest($2::text[],$3::text[]) refs(kind,value) ON e.ref_kind=refs.kind AND e.ref_value=refs.value WHERE e.subject_id=$1 AND e.use_kind<>'presented' GROUP BY e.ref_kind,e.ref_value,e.use_kind ORDER BY e.ref_kind,e.ref_value,e.use_kind")
            .bind(subject.0).bind(kinds).bind(values).fetch_all(self.0.store.pool()).await.map_err(nous_persistence::database_error)?;
        let mut result = std::collections::BTreeMap::<String, Vec<k::UseSummary>>::new();
        for row in rows {
            let key = format!(
                "{}:{}",
                row.get::<String, _>("ref_kind"),
                row.get::<String, _>("ref_value")
            );
            result.entry(key).or_default().push(k::UseSummary {
                kind: row.get("use_kind"),
                count: row.get::<i64, _>("count") as u64,
                last_used_at: Some(timestamp(row.get("last_used"))),
            });
        }
        Ok(result)
    }
}

fn consolidation_query(subject: SubjectId, cue: String, limit: usize) -> CognitiveQuery {
    let constraints = QueryConstraints::default();
    CognitiveQuery {
        // This cue is quoted source content for similarity lookup, not a user intent.
        projection: ResultProjection {
            domains: vec![ResultDomain::Memory, ResultDomain::Schema],
        },
        temporal_frame: Default::default(),

        work_context: None,
        subject,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue { text: cue })],
            constraints,
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: ResultNeed {
            limit,
            ..Default::default()
        },
        effort: CognitiveEffort::Light,
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consolidation_source_lookup_does_not_require_closing_pronouns_in_quoted_content() {
        let query = consolidation_query(
            SubjectId::new(),
            "Lin said this was her preferred trial rule.".into(),
            8,
        );
        assert!(query.validate().is_ok());
    }
}
