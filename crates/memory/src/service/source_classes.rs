// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::collections::{HashMap, HashSet};
impl MemoryService {
    pub(crate) async fn source_objects_for_views(
        &self,
        subject: SubjectId,
        views: &HashMap<Uuid, MemoryView>,
        historical: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<HashMap<Uuid, Vec<CognitiveRef>>> {
        let references = |view: &MemoryView| {
            view.basis
                .iter()
                .filter_map(|basis| {
                    if let RevisionBasis::Evidence(e) = basis {
                        Some(e)
                    } else {
                        None
                    }
                })
                .flat_map(|e| {
                    let mut refs = vec![CognitiveRef::Occurrence(e.occurrence_id)];
                    match e.locator {
                        EvidenceLocator::DerivedRepresentation(id) => {
                            refs.push(CognitiveRef::DerivedRepresentation(id))
                        }
                        EvidenceLocator::DerivedRegion(id) => {
                            refs.push(CognitiveRef::DerivedRegion(id))
                        }
                        EvidenceLocator::WholeOccurrence | EvidenceLocator::SourceRegion(_) => {}
                    }
                    refs
                })
                .collect::<Vec<_>>()
        };
        let refs = views
            .values()
            .flat_map(references)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut facts = HashMap::new();
        for batch in refs.chunks(256) {
            facts.extend(
                self.material
                    .source_objects_batch(subject, batch, historical)
                    .await?,
            );
        }
        Ok(views
            .iter()
            .map(|(id, view)| {
                (
                    *id,
                    references(view)
                        .iter()
                        .filter_map(|reference| facts.get(reference))
                        .flatten()
                        .cloned()
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect(),
                )
            })
            .collect())
    }
}
