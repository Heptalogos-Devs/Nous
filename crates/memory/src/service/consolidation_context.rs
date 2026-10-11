// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use std::collections::BTreeSet;
impl MemoryService {
    pub async fn consolidation_catalog_basis(
        &self,
        subject: SubjectId,
        scopes: &[CognitiveRef],
    ) -> Result<Vec<RevisionBasis>> {
        let mut allowed = std::collections::BTreeMap::new();
        let mut visited = BTreeSet::new();
        let mut pending = scopes.to_vec();
        while let Some(reference) = pending.pop() {
            if !visited.insert(reference.to_string()) {
                continue;
            }
            let direct = RevisionBasis::CognitionDependency(CognitionDependency {
                epistemic_relation: None,
                target_revision: reference.clone(),
                basis_role: BasisRole::Direct,
            });
            allowed.insert(direct.canonical_key(), direct);
            let basis = match reference {
                CognitiveRef::EpisodeRevision(id) => {
                    let episode = self.episode_revision(subject, id).await?;
                    allowed.extend(
                        episode
                            .members
                            .iter()
                            .filter_map(member_basis)
                            .map(|basis| (basis.canonical_key(), basis)),
                    );
                    episode.basis
                }
                CognitiveRef::JournalRevision(id) => self
                    .journal_revision(subject, id)
                    .await?
                    .points
                    .into_iter()
                    .flat_map(|point| point.basis)
                    .collect(),
                CognitiveRef::MemoryRevision(id) => self.load_basis(id).await?,
                CognitiveRef::CognitiveSchemaRevision(id) => self
                    .schema_links(subject, id)
                    .await?
                    .into_iter()
                    .map(|link| link.basis)
                    .collect(),
                _ => {
                    return Err(Error::new(
                        nous_core::ErrorOwner::Memory,
                        nous_core::ErrorCode::InvalidRequest,
                        "Consolidation dependency is not an exact cognition revision",
                    ));
                }
            };
            for basis in basis {
                allowed.insert(basis.canonical_key(), basis.clone());
                if let RevisionBasis::CognitionDependency(dependency) = basis {
                    pending.push(dependency.target_revision);
                }
            }
        }
        Ok(allowed.into_values().collect())
    }
}
fn member_basis(member: &EpisodeMember) -> Option<RevisionBasis> {
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
