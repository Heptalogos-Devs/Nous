// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::HashSet;
impl MemoryService {
    pub async fn consolidation_catalog_basis(
        &self,
        subject: SubjectId,
        scopes: &[CognitiveRef],
    ) -> Result<Vec<RevisionBasis>> {
        let mut allowed = std::collections::BTreeMap::new();
        let mut visited = HashSet::new();
        let mut pending = scopes.to_vec();
        let mut tx = self.store.read_view().await?;
        while !pending.is_empty() {
            let references = pending
                .drain(..pending.len().min(256))
                .filter(|r| visited.insert(r.clone()))
                .collect::<Vec<_>>();
            for (reference, support) in self
                .exact_supports_in(&mut tx, subject, &references, None)
                .await?
            {
                let direct = RevisionBasis::CognitionDependency(CognitionDependency {
                    epistemic_relation: None,
                    target_revision: reference,
                    basis_role: BasisRole::Direct,
                });
                allowed.insert(direct.canonical_key(), direct);
                allowed.extend(
                    support
                        .members
                        .iter()
                        .filter_map(member_basis)
                        .map(|basis| (basis.canonical_key(), basis)),
                );
                pending.extend(support.basis.iter().filter_map(|basis| match basis {
                    RevisionBasis::CognitionDependency(dependency) => {
                        Some(dependency.target_revision.clone())
                    }
                    _ => None,
                }));
                allowed.extend(
                    support
                        .basis
                        .into_iter()
                        .map(|basis| (basis.canonical_key(), basis)),
                );
            }
        }
        tx.commit().await.map_err(db)?;
        Ok(allowed.into_values().collect())
    }
}
pub(super) fn member_basis(member: &EpisodeMember) -> Option<RevisionBasis> {
    match member.reference {
        CognitiveRef::Occurrence(id) => Some(RevisionBasis::Evidence(EvidenceRef {
            epistemic_relation: None,
            occurrence_id: id,
            locator: EvidenceLocator::WholeOccurrence,
            basis_role: BasisRole::Direct,
        })),
        _ => None,
    }
}

pub struct ConsolidationMetadata {
    pub object_id: Uuid,
    pub revisions: Vec<Uuid>,
    pub schema_context: Option<(String, String)>,
}

impl MemoryService {
    /// Capture owner identity, all sibling revisions and Schema presentation once for a candidate set.
    pub async fn consolidation_metadata(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
    ) -> Result<std::collections::BTreeMap<String, ConsolidationMetadata>> {
        let (kinds, values): (Vec<_>, Vec<_>) = references.iter().map(reference_parts).unzip();
        let rows = sqlx::query("WITH requested AS (SELECT * FROM unnest($2::text[],$3::text[]) q(kind,value)) SELECT q.kind,q.value,r.memory_id AS object_id,ARRAY(SELECT memory_revision_id FROM memory_revisions WHERE memory_id=r.memory_id) AS revisions,NULL::text AS formation,NULL::text AS context FROM requested q JOIN memory_revisions r ON q.kind='memory_revision' AND r.memory_revision_id::text=q.value WHERE r.subject_id=$1 UNION ALL SELECT q.kind,q.value,r.schema_id,ARRAY(SELECT schema_revision_id FROM cognitive_schema_revisions WHERE schema_id=r.schema_id),r.formation_kind,'Title: '||COALESCE(r.title,'')||E'\nClaim: '||r.structural_claim||E'\nApplicability: '||r.applicability_description||E'\nBoundary: '||r.boundary_definition||E'\nTags: '||r.tags::text FROM requested q JOIN cognitive_schema_revisions r ON q.kind='cognitive_schema_revision' AND r.schema_revision_id::text=q.value JOIN cognitive_schemas o USING(schema_id) WHERE o.subject_id=$1")
            .bind(subject.0).bind(kinds).bind(values).fetch_all(self.store.pool()).await.map_err(db)?;
        rows.into_iter()
            .map(|row| {
                let key = format!(
                    "{}:{}",
                    row.try_get::<String, _>("kind").map_err(db)?,
                    row.try_get::<String, _>("value").map_err(db)?
                );
                let formation: Option<String> = row.try_get("formation").map_err(db)?;
                let context: Option<String> = row.try_get("context").map_err(db)?;
                Ok((
                    key,
                    ConsolidationMetadata {
                        object_id: row.try_get("object_id").map_err(db)?,
                        revisions: row.try_get("revisions").map_err(db)?,
                        schema_context: formation.zip(context),
                    },
                ))
            })
            .collect()
    }

    pub async fn consolidation_named_entities(
        &self,
        subject: SubjectId,
        cue: &str,
        limit: usize,
    ) -> Result<Vec<String>> {
        sqlx::query_scalar("SELECT DISTINCT b.canonical_ref FROM lexical_bindings b JOIN lexical_visibility v USING(lexical_ref) WHERE v.subject_id=$1 AND b.object_kind='entity' AND b.tombstoned_at IS NULL AND ((length(v.display_name)>1 AND strpos(lower($2),lower(v.display_name))>0) OR EXISTS(SELECT 1 FROM unnest(v.aliases) alias WHERE length(alias)>1 AND strpos(lower($2),lower(alias))>0)) ORDER BY b.canonical_ref LIMIT $3")
            .bind(subject.0).bind(cue).bind(i64::try_from(limit).map_err(|_| Error::Invalid("entity catalog limit exceeded".into()))?).fetch_all(self.store.pool()).await.map_err(db)
    }
}
