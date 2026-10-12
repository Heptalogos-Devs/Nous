// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

/// Memory-owned context content after current lifecycle and accessibility checks.
#[derive(Debug)]
pub struct MemoryContextProjection {
    pub source_revision: MemoryRevisionId,
    pub text: Option<String>,
    pub evidence: Vec<EvidenceHandle>,
}

impl MemoryService {
    pub async fn context_projection(
        &self,
        subject: SubjectId,
        reference: &CognitiveRef,
        policy: &AccessibilityPolicy,
        now: DateTime<Utc>,
        effort: CognitiveEffort,
        explicit: bool,
        max_bytes: usize,
    ) -> Result<MemoryContextProjection> {
        let revision = match reference {
            CognitiveRef::Memory(id) => sqlx::query_scalar::<_, Uuid>(
                "SELECT current_revision_id FROM memory_objects WHERE subject_id=$1 AND memory_id=$2",
            )
            .bind(subject.0)
            .bind(id.0)
            .fetch_optional(self.store.pool())
            .await
            .map_err(db)?
            .ok_or_else(|| Error::NotFound("memory not found".into()))?,
            CognitiveRef::MemoryRevision(id) => id.0,
            _ => return Err(Error::Invalid("Memory context requires a Memory reference".into())),
        };
        let memory = self
            .memory_views(subject, &[revision], policy, None, true, now)
            .await?
            .remove(&revision)
            .ok_or_else(|| Error::NotFound("memory not found".into()))?;
        if memory.object.acceptance_state != AcceptanceState::Accepted {
            return Err(Error::Unavailable("Memory is withdrawn".into()));
        }
        if memory.object.integrity_state != IntegrityState::Valid {
            return Err(Error::Unavailable("Memory requires revalidation".into()));
        }
        if memory.object.suppression_state != SuppressionState::Normal {
            return Err(Error::Unavailable("Memory is suppressed".into()));
        }
        if !accessibility_eligible(memory.accessibility_level, effort, explicit) {
            return Err(Error::Unavailable(
                "Memory accessibility requires deeper or explicit recall".into(),
            ));
        }
        let evidence = memory
            .basis
            .into_iter()
            .map(|basis| match basis {
                RevisionBasis::Evidence(value) => EvidenceHandle {
                    reference: value.cognitive_ref(),
                    epistemic_relation: value.epistemic_relation,
                    basis_role: value.basis_role.as_str().into(),
                },
                RevisionBasis::CognitionDependency(value) => EvidenceHandle {
                    reference: value.target_revision,
                    epistemic_relation: value.epistemic_relation,
                    basis_role: value.basis_role.as_str().into(),
                },
                RevisionBasis::Seed(value) => EvidenceHandle {
                    reference: CognitiveRef::CognitiveSeedVersion(value.seed_version_id),
                    epistemic_relation: None,
                    basis_role: "seed".into(),
                },
            })
            .collect();
        let text = (max_bytes > 0).then(|| {
            let mut text = memory.revision.representation_text;
            let mut end = text.len().min(max_bytes);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text
        });
        Ok(MemoryContextProjection {
            source_revision: memory.revision.memory_revision_id,
            text,
            evidence,
        })
    }
}
