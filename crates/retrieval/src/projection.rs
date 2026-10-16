// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::ProjectionEdgeProvenance;
use std::collections::{BTreeSet, HashMap, HashSet};

pub struct OwnedTopologyProjection {
    pub watermark: i64,
    pub nodes: Vec<CognitiveRef>,
    pub edges: Vec<WaveEdgeEvidence>,
}

/// SQL supplies rows; semantic owners supply their view-aware source meaning.
pub struct VcpProjectionInput {
    pub authority_watermark: i64,
    pub watermarks: nous_persistence::ProjectionWatermarks,
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

    pub async fn topology_projection(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<OwnedTopologyProjection> {
        let topology = self
            .store
            .topology_projection_input(subject, memory_enabled, view)
            .await?;
        let references: HashSet<_> = topology
            .edges
            .iter()
            .filter_map(|edge| match &edge.provenance {
                ProjectionEdgeProvenance::AuthorityBasis(reference) => Some(reference.clone()),
                _ => None,
            })
            .collect();
        let mut roots = HashMap::new();
        for reference in references {
            let contribution = if let Some(memory) = self.memory.as_ref().filter(|_| memory_enabled)
            {
                memory
                    .provenance_for_reference(subject, &reference, view)
                    .await?
            } else {
                BTreeSet::new()
            };
            roots.insert(reference, contribution);
        }
        if view.is_none()
            && self
                .store
                .projection_watermark(subject, "topology", "")
                .await?
                != topology.watermark
        {
            return Err(Error::Conflict(
                "Topology changed while reading owner contributions".into(),
            ));
        }
        Ok(OwnedTopologyProjection {
            watermark: topology.watermark,
            nodes: topology.nodes,
            edges: expand_edges(topology.edges, &roots)?,
        })
    }

    pub async fn vcp_projection_input(
        &self,
        subject: SubjectId,
        memory_enabled: bool,
        budget: nous_persistence::EpisodeTextBudget,
        space: &str,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<VcpProjectionInput> {
        let rows = self
            .store
            .vcp_projection_rows(subject, memory_enabled, budget, space, view)
            .await?;
        let (authority_watermark, watermarks, sources, topology) = (
            rows.authority_watermark,
            rows.watermarks,
            rows.sources,
            rows.topology,
        );
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
        let material_refs: Vec<_> = references
            .iter()
            .filter(|reference| nous_runtime::CognitiveContributor::owns(&self.material, reference))
            .cloned()
            .collect();
        for batch in material_refs.chunks(256) {
            roots.extend(
                self.material
                    .lineage_roots_batch(subject, batch, view)
                    .await?,
            );
        }
        for reference in references {
            if roots.contains_key(&reference) {
                continue;
            }
            let contribution = if let Some(memory) = self.memory.as_ref().filter(|_| memory_enabled)
            {
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
        let edges = expand_edges(topology.edges, &roots)?;
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
        Ok(VcpProjectionInput {
            authority_watermark,
            watermarks,
            sources,
            topology: OwnedTopologyProjection {
                watermark: topology.watermark,
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
        let fragment_refs: Vec<_> = sources
            .iter()
            .filter(|source| {
                if nous_runtime::CognitiveContributor::owns(&self.material, &source.reference) {
                    selected.contains_key(&source.reference)
                } else {
                    source.text.is_some()
                }
            })
            .flat_map(|source| {
                source
                    .member_fragments
                    .iter()
                    .map(|fragment| fragment.reference.clone())
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let mut fragments = HashMap::new();
        let mut fragment_text = HashMap::new();
        let mut unresolved = Vec::new();
        for reference in fragment_refs {
            if let Some(input) = selected.get(&reference) {
                let end = input
                    .text
                    .floor_char_boundary(budget.fragment_max_bytes.min(input.text.len()));
                fragment_text.insert(reference, Some(input.text[..end].to_owned()));
            } else {
                unresolved.push(reference);
            }
        }
        for batch in unresolved.chunks(256) {
            fragments.extend(self.material.read_inputs(subject, batch, view).await?);
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
                if !fragment_text.contains_key(&fragment.reference) {
                    let member = match fragments.remove(&fragment.reference) {
                        Some(input) => self
                            .material
                            .read_text_input(input, budget.fragment_max_bytes as u64)
                            .await?
                            .map(|view| view.text),
                        None => None,
                    };
                    fragment_text.insert(fragment.reference.clone(), member);
                }
                let Some(Some(member)) = fragment_text.get(&fragment.reference) else {
                    continue;
                };
                let remaining = budget.total_max_bytes.saturating_sub(member_bytes + 1);
                if remaining == 0 {
                    break;
                }
                let end = member.floor_char_boundary(remaining.min(member.len()));
                member_bytes += end + 1;
                text.push('\n');
                text.push_str(&member[..end]);
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

fn expand_edges(
    edges_input: Vec<nous_persistence::TopologyEdgeSource>,
    roots: &HashMap<CognitiveRef, BTreeSet<EvidenceRoot>>,
) -> Result<Vec<WaveEdgeEvidence>> {
    let mut edges = Vec::new();
    for edge in edges_input {
        let source_refs = match &edge.provenance {
            ProjectionEdgeProvenance::Structure(_) => vec![edge.from.clone(), edge.to.clone()],
            ProjectionEdgeProvenance::AuthorityBasis(reference) => vec![reference.clone()],
        };
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
                source_refs: source_refs.clone(),
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
    Ok(edges)
}
