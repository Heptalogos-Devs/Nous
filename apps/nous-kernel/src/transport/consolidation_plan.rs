// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::*;
use std::collections::BTreeSet;

impl KernelService {
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
        let references: Vec<_> = hits
            .iter()
            .map(|hit| hit.revision.as_ref().unwrap_or(&hit.reference).clone())
            .collect();
        let metadata = self
            .require_memory()?
            .consolidation_metadata(subject, &references)
            .await?;
        let mut use_summaries = self.0.cognition.use_summaries(subject, &references).await?;
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
            let state = metadata.get(&reference.to_string()).ok_or_else(|| {
                Error::NotFound("consolidation candidate no longer exists".into())
            })?;
            let (formation, text) = state.schema_context.clone().unwrap_or_else(|| {
                (
                    hit.formation_mode.unwrap_or_default(),
                    hit.representation.unwrap_or_default(),
                )
            });
            let text_truncated = text.chars().count() > policy.candidate_text_chars;
            let text: String = text.chars().take(policy.candidate_text_chars).collect();
            let uses = use_summaries
                .remove(&reference.to_string())
                .unwrap_or_default();
            plan.candidates.push(k::ConsolidationCandidate {
                key: reference.to_string(),
                target: Some(k::ExpectedCognition {
                    object_id: state.object_id.to_string(),
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
                r#use: uses
                    .into_iter()
                    .map(|value| k::UseSummary {
                        kind: value.kind,
                        count: value.count,
                        last_used_at: Some(timestamp(value.last_used_at)),
                    })
                    .collect(),
                valid_time: Some(temporal_proto(&hit.freshness.valid_time)),
                formed_at: hit.freshness.formed_at.map(timestamp),
                recorded_at: hit.freshness.recorded_at.map(timestamp),
            });
        }
        plan.max_consolidation_actions =
            snapshot.get(nous_memory::CONSOLIDATION_MAX_ACTIONS)? as u32;
        self.finish_consolidation_catalog(plan, subject, &scopes, entities, &policy, &metadata)
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
        self.require_memory()?
            .consolidation_named_entities(subject, &cue, limit)
            .await
    }
    async fn finish_consolidation_catalog(
        &self,
        plan: &mut k::MaintenancePlan,
        subject: SubjectId,
        scopes: &[CognitiveRef],
        entities: BTreeSet<String>,
        policy: &nous_memory::ConsolidationContextPolicy,
        metadata: &std::collections::BTreeMap<String, nous_memory::ConsolidationMetadata>,
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
            let revisions = &metadata
                .get(&reference.to_string())
                .ok_or_else(|| Error::NotFound("consolidation candidate no longer exists".into()))?
                .revisions;
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
