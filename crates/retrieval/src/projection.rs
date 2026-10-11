// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::ProjectionEdgeProvenance;
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct OwnedTopologyProjection {
    pub nodes: Vec<CognitiveRef>,
    pub edges: Vec<WaveEdgeEvidence>,
}

/// SQL supplies rows; semantic owners supply their view-aware source meaning.
pub struct CognitiveProjectionInput {
    pub authority_watermark: i64,
    pub topology: OwnedTopologyProjection,
    pub sources: Vec<nous_persistence::TextProjectionSource>,
    pub evidence_roots: HashMap<CognitiveRef, HashSet<String>>,
}

impl ServingService {
    pub(crate) async fn documents_lookup(
        &self,
        subject: SubjectId,
        references: &[CognitiveRef],
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<LexicalDocument>> {
        let budget = self
            .configuration
            .snapshot_for_subject(subject)?
            .get(crate::EPISODE_SYNOPSIS)?;
        let enabled = self.projection_capabilities(subject).await?.memory;
        let sources = self
            .store
            .text_projection_lookup(subject, references, enabled, budget, view)
            .await?;
        self.documents(subject, sources, budget, view).await
    }

    pub async fn projection_input(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        budget: nous_persistence::EpisodeTextBudget,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<CognitiveProjectionInput> {
        let (authority_watermark, sources, topology) = match view {
            Some(view) => {
                if view.subject != subject {
                    return Err(Error::Invalid("projection view subject mismatch".into()));
                }
                let rows = self.store.historical_projection_input(view, budget).await?;
                (0, rows.sources, rows.topology)
            }
            None => {
                let rows = self
                    .store
                    .cognitive_projection_rows(subject, memory_enabled, budget)
                    .await?;
                (rows.authority_watermark, rows.sources, rows.topology)
            }
        };
        let references: HashSet<_> = sources
            .iter()
            .map(|source| source.reference.clone())
            .chain(
                topology
                    .edges
                    .iter()
                    .filter_map(|edge| match &edge.provenance {
                        ProjectionEdgeProvenance::AuthorityBasis(reference) => {
                            Some(reference.clone())
                        }
                        ProjectionEdgeProvenance::Structure(_) => None,
                    }),
            )
            .collect();
        let mut roots = HashMap::<CognitiveRef, BTreeSet<EvidenceRoot>>::new();
        for reference in references {
            let contribution =
                if nous_runtime::CognitiveContributor::owns(&self.material, &reference) {
                    self.material
                        .lineage_roots(subject, &reference, view)
                        .await?
                } else if let Some(memory) = self.memory.as_ref().filter(|_| memory_enabled) {
                    memory
                        .provenance_for_reference(subject, &reference, view)
                        .await?
                } else {
                    BTreeSet::new()
                };
            roots.insert(reference, contribution);
        }
        if view.is_none() && self.store.authority_seq(subject).await? != authority_watermark {
            return Err(Error::Conflict(
                "Authority changed while reading owner projection contributions".into(),
            ));
        }
        let mut edges = Vec::new();
        for edge in topology.edges {
            let origins: Vec<_> = match edge.provenance {
                ProjectionEdgeProvenance::Structure(identity) => vec![Some(identity)],
                ProjectionEdgeProvenance::AuthorityBasis(reference) => {
                    let contribution = roots.get(&reference).ok_or_else(|| {
                        Error::Infrastructure("owner provenance contribution missing".into())
                    })?;
                    if contribution.is_empty() {
                        vec![None]
                    } else {
                        contribution
                            .iter()
                            .map(|root| Some(root.root_key.clone()))
                            .collect()
                    }
                }
            };
            for provenance_root in origins {
                edges.push(WaveEdgeEvidence {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    basis_class: edge.basis_class.clone(),
                    association_kind: edge.association_kind.clone(),
                    polarity: edge.polarity.clone(),
                    support_mass: edge.support_mass,
                    provenance_root,
                });
            }
        }
        let evidence_roots = roots
            .into_iter()
            .map(|(reference, roots)| {
                (
                    reference,
                    roots
                        .into_iter()
                        .filter(|root| root.certainty == EvidenceRootCertainty::Known)
                        .map(|root| root.root_key)
                        .collect(),
                )
            })
            .collect();
        Ok(CognitiveProjectionInput {
            authority_watermark,
            sources,
            topology: OwnedTopologyProjection {
                nodes: topology.nodes,
                edges,
            },
            evidence_roots,
        })
    }
}

impl ServingService {
    pub(crate) async fn documents(
        &self,
        subject: SubjectId,
        sources: Vec<nous_persistence::TextProjectionSource>,
        budget: nous_persistence::EpisodeTextBudget,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<Vec<LexicalDocument>> {
        let material_refs: Vec<_> = sources
            .iter()
            .filter(|source| {
                nous_runtime::CognitiveContributor::owns(&self.material, &source.reference)
            })
            .map(|source| source.reference.clone())
            .collect();
        let mut selected = HashMap::new();
        for batch in material_refs.chunks(256) {
            selected.extend(
                self.material
                    .document_text_views(subject, batch, view)
                    .await?,
            );
        }
        let mut documents = Vec::new();
        for source in sources {
            let mut text =
                if nous_runtime::CognitiveContributor::owns(&self.material, &source.reference) {
                    let Some(input) = selected.remove(&source.reference) else {
                        continue;
                    };
                    input.text
                } else if let Some(text) = source.text {
                    text
                } else {
                    continue;
                };
            let mut member_bytes = 0;
            for fragment in source.member_fragments {
                let member = self
                    .material
                    .text_excerpt(
                        subject,
                        &fragment.reference,
                        budget.fragment_max_bytes as u64,
                        view,
                    )
                    .await?
                    .map(|view| view.text);
                let Some(mut member) = member else {
                    continue;
                };
                let remaining = budget.total_max_bytes.saturating_sub(member_bytes + 1);
                if remaining == 0 {
                    break;
                }
                member.truncate(member.floor_char_boundary(remaining.min(member.len())));
                member_bytes += member.len() + 1;
                text.push('\n');
                text.push_str(&member);
            }
            documents.push(LexicalDocument {
                serving_doc_id: documents.len() as u64,
                reference: source.reference,
                representation_text: text,
                title: source.title,
                entity_refs: source.entity_refs,
                tag_ids: source.tag_ids,
                schema_ids: source.schema_ids,
                source_class: source.source_class,
            });
        }
        Ok(documents)
    }
}
