//! Nous identity and evidence mapping for the independent reference kernels.
//! Inputs come from one Authority projection; numerical IDs are generation-local.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::WaveEdgeEvidence;
use crate::reference::{ReferenceCurve, ReferenceCurveTag};
use nous_core::{CognitiveRef, Error, Result};
use nous_runtime::LaneCandidate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VcpIdentityMap {
    references: Vec<CognitiveRef>,
}
impl VcpIdentityMap {
    pub fn new(references: impl IntoIterator<Item = CognitiveRef>) -> Self {
        let mut references = references.into_iter().collect::<Vec<_>>();
        references.sort_by_key(ToString::to_string);
        references.dedup();
        Self { references }
    }
    pub fn id(&self, reference: &CognitiveRef) -> Result<i64> {
        let key = reference.to_string();
        self.references
            .binary_search_by(|r| r.to_string().cmp(&key))
            .ok()
            .map(|i| i as i64 + 1)
            .ok_or_else(|| {
                Error::Invalid("VCP reference is outside the generation identity map".into())
            })
    }
    pub fn reference(&self, id: i64) -> Result<&CognitiveRef> {
        if id <= 0 {
            return Err(Error::Invalid("VCP node ID must be positive".into()));
        }
        let index = usize::try_from(id - 1)
            .map_err(|_| Error::Invalid("VCP node ID must be positive".into()))?;
        self.references.get(index).ok_or_else(|| {
            Error::Invalid("VCP output ID is outside the generation identity map".into())
        })
    }
    pub fn references(&self) -> &[CognitiveRef] {
        &self.references
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VcpCurveOrder {
    /// An actual source sequence supplied by the projection owner.
    SourceSequence,
    /// Authority membership has no sequence; use a reproducible control order.
    StableIdentity,
}
#[derive(Debug, Clone)]
pub struct VcpCurveMember {
    pub reference: CognitiveRef,
    pub vector: Vec<f32>,
}
#[derive(Debug, Clone)]
pub struct VcpCurveSource {
    pub reference: CognitiveRef,
    pub vector: Vec<f32>,
    pub members: Vec<VcpCurveMember>,
    pub order: VcpCurveOrder,
}
pub fn vcp_candidate_curve(
    ids: &VcpIdentityMap,
    source: &VcpCurveSource,
) -> Result<ReferenceCurve> {
    if source.vector.is_empty() || source.vector.iter().any(|v| !v.is_finite()) {
        return Err(Error::Invalid(
            "VCP candidate requires a finite embedding vector".into(),
        ));
    }
    let mut members = source
        .members
        .iter()
        .map(|m| {
            if m.vector.len() != source.vector.len() || m.vector.iter().any(|v| !v.is_finite()) {
                return Err(Error::Invalid(
                    "VCP curve member embedding does not match candidate dimension".into(),
                ));
            }
            Ok((ids.id(&m.reference)?, m.vector.clone()))
        })
        .collect::<Result<Vec<_>>>()?;
    if matches!(source.order, VcpCurveOrder::StableIdentity) {
        members.sort_by_key(|(id, _)| *id);
    }
    let mut seen = BTreeSet::new();
    members.retain(|(id, _)| seen.insert(*id));
    let tags = members
        .into_iter()
        .enumerate()
        .map(|(i, (id, vector))| ReferenceCurveTag {
            id,
            vector,
            position: i as i64 + 1,
        })
        .collect();
    Ok(ReferenceCurve {
        id: ids.id(&source.reference)?,
        chunk_vector: source.vector.clone(),
        tags,
    })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct VcpEvidenceContribution {
    pub source_id: i64,
    pub target_id: i64,
    pub provenance_root: Option<String>,
    pub basis_class: String,
    pub association_kind: String,
    pub support_mass: f64,
}
/// Keep one maximum per evidence root and semantic edge identity. Negative
/// evidence remains explicit in the projection, but supplies no positive flow.
pub fn vcp_evidence_contributions(
    ids: &VcpIdentityMap,
    edges: &[WaveEdgeEvidence],
) -> Result<Vec<VcpEvidenceContribution>> {
    let mut contributions = BTreeMap::<(i64, i64, Option<String>, String, String), f64>::new();
    for e in edges {
        if e.polarity != "positive" || !e.support_mass.is_finite() || e.support_mass <= 0.0 {
            continue;
        }
        let from = ids.id(&e.from)?;
        let to = ids.id(&e.to)?;
        if from == to {
            continue;
        }
        let root = e.provenance_root.clone();
        let key = (
            from,
            to,
            root,
            e.basis_class.clone(),
            e.association_kind.clone(),
        );
        contributions
            .entry(key)
            .and_modify(|mass| *mass = mass.max(e.support_mass))
            .or_insert(e.support_mass);
    }
    Ok(contributions
        .into_iter()
        .map(
            |(
                (source_id, target_id, provenance_root, basis_class, association_kind),
                support_mass,
            )| VcpEvidenceContribution {
                source_id,
                target_id,
                provenance_root,
                basis_class,
                association_kind,
                support_mass,
            },
        )
        .collect())
}

pub fn vcp_lane_candidates(
    ids: &VcpIdentityMap,
    ranked: &[(i64, serde_json::Value)],
    profile: &str,
    limit: usize,
) -> Result<Vec<LaneCandidate>> {
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for (id, metadata) in ranked {
        let reference = ids.reference(*id)?.clone();
        if !seen.insert(*id) {
            continue;
        }
        if candidates.len() >= limit {
            break;
        }
        candidates.push(LaneCandidate {
            reference,
            rank: candidates.len() as u32 + 1,
            variants: vec![format!("topology:{profile}")],
            provider_metadata: metadata.clone(),
        });
    }
    Ok(candidates)
}

#[cfg(test)]
#[path = "../../tests/unit/vcp_adapter.rs"]
mod tests;
