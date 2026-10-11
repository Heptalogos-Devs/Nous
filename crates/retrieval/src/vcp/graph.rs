//! Generation graph assembly from coherent Nous projection material.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::reference::{
    ReferenceFileTags, ReferenceGraphConfig, ReferenceGraphInput, ReferenceGraphOutput,
    ReferenceProvenanceEdge, reference_graph_file_facts, reference_graph_from_facts,
};
use crate::{
    VcpCurveOrder, VcpEvidenceContribution, VcpIdentityMap, VcpProjectedDocument,
    VcpProjectionMaterial, vcp_evidence_contributions,
};
use nous_core::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize, Deserialize)]
pub struct VcpGraphAssets {
    pub graph: ReferenceGraphOutput,
    pub membership: Vec<ReferenceFileTags>,
    /// Root IDs have their own namespace. They are evidence identities, not
    /// candidate IDs or an Authority visibility filter.
    pub provenance_roots: Vec<String>,
    pub evidence: Vec<VcpEvidenceContribution>,
}

pub fn vcp_graph_assets(
    material: &VcpProjectionMaterial,
    pairwise: &[(i64, i64, f64)],
    anchor_gain: &[(i64, f64)],
    config: &ReferenceGraphConfig,
) -> Result<VcpGraphAssets> {
    let ids = &material.identities;
    let evidence = vcp_evidence_contributions(ids, &material.edges)?;
    let mut facts = BTreeMap::<(i64, i64), f64>::new();
    let mut roots = BTreeMap::<(i64, i64), BTreeMap<String, f64>>::new();
    let membership = vcp_membership(ids, &material.documents)?;
    let output = reference_graph_file_facts(&ReferenceGraphInput {
        files: membership.clone(),
        pairwise: pairwise.to_vec(),
        anchor_gain: anchor_gain.to_vec(),
        config: config.clone(),
    })?;
    for file in output {
        let reference = ids.reference(file.file_id)?;
        let document = material
            .documents
            .iter()
            .find(|document| &document.reference == reference)
            .ok_or_else(|| {
                nous_core::Error::Infrastructure("VCP membership source missing".into())
            })?;
        for (from, to, mass) in file.facts {
            *facts.entry((from, to)).or_default() += mass;
            for root in &document.evidence_roots {
                let contribution = roots
                    .entry((from, to))
                    .or_default()
                    .entry(root.clone())
                    .or_default();
                *contribution = contribution.max(mass / document.evidence_roots.len() as f64);
            }
        }
    }
    for item in &evidence {
        let key = (item.source_id, item.target_id);
        *facts.entry(key).or_default() += item.support_mass;
        // Unrooted support may affect flow; it does not invent an independent source identity.
        if let Some(root) = &item.provenance_root {
            let contribution = roots
                .entry(key)
                .or_default()
                .entry(root.clone())
                .or_default();
            *contribution = contribution.max(item.support_mass);
        }
    }
    let provenance_roots = roots
        .values()
        .flat_map(|r| r.keys().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let root_ids = provenance_roots
        .iter()
        .enumerate()
        .map(|(i, root)| (root.clone(), i as i64 + 1))
        .collect::<BTreeMap<_, _>>();
    let provenance = roots
        .into_iter()
        .map(|((source_id, target_id), values)| ReferenceProvenanceEdge {
            source_id,
            target_id,
            file_contributions: values
                .into_iter()
                .map(|(root, mass)| (root_ids[&root], mass))
                .collect(),
        })
        .collect();
    let facts = facts
        .into_iter()
        .map(|((from, to), mass)| (from, to, mass))
        .collect::<Vec<_>>();
    let graph = reference_graph_from_facts(&facts, anchor_gain, provenance, config)?;
    Ok(VcpGraphAssets {
        graph,
        membership,
        provenance_roots,
        evidence,
    })
}

pub(crate) fn vcp_membership(
    ids: &VcpIdentityMap,
    documents: &[VcpProjectedDocument],
) -> Result<Vec<ReferenceFileTags>> {
    let mut membership = Vec::new();
    for document in documents {
        let file_id = ids.id(&document.reference)?;
        let mut seen = BTreeSet::new();
        let mut concepts = document
            .concept_refs
            .iter()
            .map(|r| ids.id(r))
            .collect::<Result<Vec<_>>>()?;
        if matches!(document.curve_order, VcpCurveOrder::StableIdentity) {
            concepts.sort_unstable();
        }
        concepts.retain(|id| seen.insert(*id));
        let file = ReferenceFileTags {
            file_id,
            tags: concepts
                .into_iter()
                .enumerate()
                .map(|(i, id)| {
                    (
                        id,
                        match document.curve_order {
                            VcpCurveOrder::SourceSequence => i as i64 + 1,
                            VcpCurveOrder::StableIdentity => 0,
                        },
                    )
                })
                .collect(),
        };
        membership.push(file);
    }
    Ok(membership)
}

#[cfg(test)]
#[path = "../../tests/unit/vcp_graph.rs"]
mod tests;
