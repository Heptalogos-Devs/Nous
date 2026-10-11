// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{reference::*, *};

#[derive(Debug, Serialize, Deserialize)]
pub struct VcpGeneration {
    pub generation_id: ServingGenerationId,
    pub cognitive_profile: nous_runtime::CognitiveProfile,
    pub authority_watermark: i64,
    pub space: EmbeddingSpaceSignature,
    pub producer: ProducerSignature,
    pub identities: VcpIdentityMap,
    pub policy: VcpAssetPolicy,
    pub vectors: Vec<(i64, Vec<f32>)>,
    pub labels: Vec<(i64, String)>,
    pub curves: Vec<ReferenceCurve>,
    pub curve_orders: Vec<(i64, VcpCurveOrder)>,
    pub candidate_evidence_roots: Vec<(i64, std::collections::BTreeSet<String>)>,
    pub pairwise: Vec<(i64, i64, f64)>,
    pub intrinsic: Vec<ReferenceIntrinsicResult>,
    pub epa: ReferenceEpaTraining,
    pub graph: VcpGraphAssets,
    pub unembedded_graph_nodes: Vec<i64>,
}
impl VcpGeneration {
    pub fn build(
        generation_id: ServingGenerationId,
        cognitive_profile: nous_runtime::CognitiveProfile,
        material: VcpProjectionMaterial,
        policy: VcpAssetPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        let ids = &material.identities;
        let mut vectors = BTreeMap::new();
        let mut labels = BTreeMap::new();
        for document in &material.documents {
            let id = ids.id(&document.reference)?;
            if vectors.insert(id, document.vector.clone()).is_some() {
                return Err(Error::Invalid("duplicate VCP projected document".into()));
            }
            if matches!(document.reference, CognitiveRef::Tag(_)) {
                labels.insert(id, document.representation_text.clone());
            }
        }
        let mut curves = Vec::new();
        let mut orders = Vec::new();
        let mut pairs = std::collections::BTreeSet::new();
        for document in &material.documents {
            let members = document
                .concept_refs
                .iter()
                .map(|reference| {
                    let id = ids.id(reference)?;
                    let vector = vectors.get(&id).ok_or_else(|| {
                        Error::Unavailable("VCP curve concept has no embedding material".into())
                    })?;
                    Ok(VcpCurveMember {
                        reference: reference.clone(),
                        vector: vector.clone(),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let curve = vcp_candidate_curve(
                ids,
                &VcpCurveSource {
                    reference: document.reference.clone(),
                    vector: document.vector.clone(),
                    members,
                    order: document.curve_order,
                },
            )?;
            for (i, a) in curve.tags.iter().enumerate() {
                for b in curve.tags.iter().skip(i + 1) {
                    pairs.insert((a.id.min(b.id), a.id.max(b.id)));
                }
            }
            orders.push((curve.id, document.curve_order));
            curves.push(curve);
        }
        let pairwise = pairs
            .into_iter()
            .map(|(a, b)| (a, b, reference_cosine(&vectors[&a], &vectors[&b])))
            .collect::<Vec<_>>();
        let membership = crate::vcp::graph::vcp_membership(ids, &material.documents)?;
        let tag_vectors = labels
            .keys()
            .map(|id| (*id, vectors[id].clone()))
            .collect::<Vec<_>>();
        let intrinsic = reference_intrinsic_residual(&ReferenceIntrinsicInput {
            dimension: material.space.dimension as usize,
            tag_vectors: tag_vectors.clone(),
            files: membership,
            pairwise: pairwise.clone(),
            config: policy.intrinsic.clone(),
        });
        let anchor_gain = intrinsic
            .iter()
            .filter_map(|r| r.anchor_gain.map(|gain| (r.id, gain)))
            .collect::<Vec<_>>();
        let graph = vcp_graph_assets(&material, &pairwise, &anchor_gain, &policy.graph)?;
        let epa = reference_train_epa(&ReferenceEpaTrainingInput {
            dimension: material.space.dimension as usize,
            vectors: tag_vectors.into_iter().map(|(_, vector)| vector).collect(),
            names: labels.values().cloned().collect(),
            requested_anchors: policy.epa_anchors,
            max_basis: policy.epa_max_basis,
            samples_per_anchor: policy.epa_samples_per_anchor,
            candidate_limit: policy.epa_candidate_limit,
        });
        let unembedded_graph_nodes = graph
            .graph
            .transport
            .node_ids
            .iter()
            .filter(|id| !vectors.contains_key(id))
            .copied()
            .collect();
        let candidate_evidence_roots = material
            .documents
            .iter()
            .map(|d| {
                Ok((
                    material.identities.id(&d.reference)?,
                    d.evidence_roots.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let generation = Self {
            generation_id,
            cognitive_profile,
            authority_watermark: material.authority_watermark,
            space: material.space,
            producer: material.producer,
            identities: material.identities,
            policy,
            vectors: vectors.into_iter().collect(),
            labels: labels.into_iter().collect(),
            candidate_evidence_roots,
            curves,
            curve_orders: orders,
            pairwise,
            intrinsic,
            epa,
            graph,
            unembedded_graph_nodes,
        };
        generation.validate()?;
        Ok(generation)
    }

    pub fn validate(&self) -> Result<()> {
        self.policy.validate()?;
        if !matches!(
            self.cognitive_profile,
            nous_runtime::CognitiveProfile::VcpDtsc | nous_runtime::CognitiveProfile::VcpRiverMemo
        ) || self.space.dimension == 0
            || VcpIdentityMap::new(self.identities.references().iter().cloned()).references()
                != self.identities.references()
        {
            return Err(Error::Invalid(
                "invalid VCP generation identity/profile/space".into(),
            ));
        }
        self.graph.graph.transport.validate()?;
        if self.vectors.windows(2).any(|v| v[0].0 >= v[1].0)
            || self.labels.windows(2).any(|v| v[0].0 >= v[1].0)
        {
            return Err(Error::Invalid(
                "VCP vector/label IDs must be unique and sorted".into(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        for (id, vector) in &self.vectors {
            self.identities.reference(*id)?;
            if !seen.insert(*id)
                || vector.len() != self.space.dimension as usize
                || vector.iter().any(|v| !v.is_finite())
            {
                return Err(Error::Invalid("invalid VCP generation vector".into()));
            }
        }
        for id in &self.graph.graph.transport.node_ids {
            self.identities.reference(*id)?;
        }
        for (id, _) in &self.labels {
            if !seen.contains(id)
                || !matches!(self.identities.reference(*id)?, CognitiveRef::Tag(_))
            {
                return Err(Error::Invalid(
                    "VCP label must identify an embedded Tag".into(),
                ));
            }
        }
        let vectors = self
            .vectors
            .iter()
            .map(|(id, vector)| (*id, vector))
            .collect::<BTreeMap<_, _>>();
        let mut curve_ids = std::collections::BTreeSet::new();
        for curve in &self.curves {
            self.identities.reference(curve.id)?;
            if !curve_ids.insert(curve.id)
                || vectors.get(&curve.id).copied() != Some(&curve.chunk_vector)
            {
                return Err(Error::Invalid(
                    "VCP curve body disagrees with generation vector".into(),
                ));
            }
            let mut tags = std::collections::BTreeSet::new();
            for tag in &curve.tags {
                self.identities.reference(tag.id)?;
                if !tags.insert(tag.id) || vectors.get(&tag.id).copied() != Some(&tag.vector) {
                    return Err(Error::Invalid(
                        "VCP curve tag disagrees with generation vector".into(),
                    ));
                }
            }
        }
        if self.candidate_evidence_roots.len() != curve_ids.len()
            || self
                .candidate_evidence_roots
                .iter()
                .map(|(id, _)| *id)
                .collect::<std::collections::BTreeSet<_>>()
                != curve_ids
        {
            return Err(Error::Invalid(
                "VCP candidate evidence identities disagree".into(),
            ));
        }
        if self.curve_orders.len() != curve_ids.len()
            || self
                .curve_orders
                .iter()
                .map(|(id, _)| *id)
                .collect::<std::collections::BTreeSet<_>>()
                != curve_ids
        {
            return Err(Error::Invalid("VCP curve order identities disagree".into()));
        }
        if self.epa.success
            && (self.epa.mean.len() != self.space.dimension as usize
                || self.epa.basis.is_empty()
                || self.epa.basis.len() != self.epa.energies.len()
                || self.epa.basis.iter().any(|axis| {
                    axis.len() != self.space.dimension as usize
                        || axis.iter().any(|v| !v.is_finite())
                })
                || self.epa.mean.iter().any(|v| !v.is_finite())
                || self.epa.energies.iter().any(|v| !v.is_finite() || *v < 0.0))
        {
            return Err(Error::Invalid("invalid VCP EPA basis".into()));
        }
        for edge in &self.graph.graph.provenance {
            self.identities.reference(edge.source_id)?;
            self.identities.reference(edge.target_id)?;
            if edge.file_contributions.iter().any(|(id, mass)| {
                *id <= 0
                    || *id as usize > self.graph.provenance_roots.len()
                    || !mass.is_finite()
                    || *mass <= 0.0
            }) {
                return Err(Error::Invalid("invalid VCP provenance root".into()));
            }
        }
        Ok(())
    }
}
