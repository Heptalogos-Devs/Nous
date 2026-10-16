// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{reference::ReferenceResidualCandidate, *};
use std::path::Path;
use usearch::ffi::MetricKind;

/// One immutable generation owns its numerical assets and both native indexes.
/// Readout borrows candidate search; only query preparation searches Tag residuals.
pub struct VcpServingGeneration {
    assets: VcpGeneration,
    candidates: DenseGeneration,
    tags: DenseGeneration,
}
impl std::ops::Deref for VcpServingGeneration {
    type Target = VcpGeneration;
    fn deref(&self) -> &Self::Target {
        &self.assets
    }
}
impl Serialize for VcpServingGeneration {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.assets.serialize(serializer)
    }
}
impl VcpServingGeneration {
    pub fn create(assets: VcpGeneration, path: &Path) -> Result<Self> {
        assets.validate()?;
        let candidates = build_index(&assets, false)?;
        let tags = build_index(&assets, true)?;
        candidates.save(&path.join("vcp-candidates.usearch"))?;
        tags.save(&path.join("vcp-tags.usearch"))?;
        Ok(Self {
            assets,
            candidates,
            tags,
        })
    }
    pub fn open(assets: VcpGeneration, path: &Path) -> Result<Self> {
        assets.validate()?;
        let candidates = DenseGeneration::open_with_metric(
            &path.join("vcp-candidates.usearch"),
            assets.generation_id,
            assets.space.clone(),
            records(&assets, false)?,
            MetricKind::Cos,
        )?;
        let tags = DenseGeneration::open_with_metric(
            &path.join("vcp-tags.usearch"),
            assets.generation_id,
            assets.space.clone(),
            records(&assets, true)?,
            MetricKind::Cos,
        )?;
        for (id, vector) in &assets.vectors {
            if candidates.vector(*id as u64) != Some(vector.as_slice()) {
                return Err(Error::Infrastructure(
                    "VCP candidate index disagrees with asset vectors".into(),
                ));
            }
        }
        for (id, _) in &assets.labels {
            if tags.vector(*id as u64) != candidates.vector(*id as u64) {
                return Err(Error::Infrastructure(
                    "VCP Tag index disagrees with candidate vectors".into(),
                ));
            }
        }
        Ok(Self {
            assets,
            candidates,
            tags,
        })
    }
    pub fn search_candidates_for_projection(
        &self,
        vector: &[f32],
        limit: usize,
        projection: &ResultProjection,
    ) -> Result<Vec<DenseMatch>> {
        let allowed = self
            .candidates
            .records()
            .filter(|record| projection.allows_reference(&record.reference))
            .map(|record| {
                u32::try_from(record.serving_doc_id).map_err(|_| {
                    Error::Invalid("VCP filtered index exceeds bitmap ID range".into())
                })
            })
            .collect::<Result<roaring::RoaringBitmap>>()?;
        self.candidates.search_filtered(vector, limit, &allowed)
    }
    pub fn search_residual_tags(
        &self,
        vector: &[f32],
        limit: usize,
    ) -> Result<Vec<ReferenceResidualCandidate>> {
        if self.tags.is_empty() {
            return Ok(Vec::new());
        }
        self.tags
            .search(vector, limit)?
            .into_iter()
            .map(|hit| {
                let id = i64::try_from(hit.serving_doc_id).map_err(|_| {
                    Error::Infrastructure("VCP index identity exceeds signed range".into())
                })?;
                let name = self
                    .assets
                    .labels
                    .binary_search_by_key(&id, |(id, _)| *id)
                    .ok()
                    .map(|i| self.assets.labels[i].1.clone())
                    .ok_or_else(|| {
                        Error::Infrastructure("VCP Tag index identity lacks label".into())
                    })?;
                let vector = self
                    .tags
                    .vector(hit.serving_doc_id)
                    .ok_or_else(|| {
                        Error::Infrastructure("VCP Tag index identity lacks vector".into())
                    })?
                    .to_vec();
                Ok(ReferenceResidualCandidate {
                    id,
                    name,
                    vector,
                    // Frozen memo_pipeline computes this in f64 from f32 distance.
                    similarity: 1.0 / (1.0 + f64::from(hit.distance)),
                })
            })
            .collect()
    }
}
fn records(assets: &VcpGeneration, tags_only: bool) -> Result<Vec<VectorRecord>> {
    assets
        .vectors
        .iter()
        .filter(|(id, _)| {
            !tags_only
                || assets
                    .labels
                    .binary_search_by_key(id, |(id, _)| *id)
                    .is_ok()
        })
        .map(|(id, _)| {
            Ok(VectorRecord {
                serving_doc_id: *id as u64,
                reference: assets.identities.reference(*id)?.clone(),
                producer_signature: assets.producer.signature_hash.clone(),
            })
        })
        .collect()
}
fn build_index(assets: &VcpGeneration, tags_only: bool) -> Result<DenseGeneration> {
    let records = records(assets, tags_only)?;
    let mut index =
        DenseGeneration::new_with_metric(assets.space.clone(), records.len(), MetricKind::Cos)?;
    index.generation_id = assets.generation_id;
    for record in records {
        let vector = assets
            .vectors
            .binary_search_by_key(&(record.serving_doc_id as i64), |(id, _)| *id)
            .ok()
            .map(|i| assets.vectors[i].1.as_slice())
            .ok_or_else(|| {
                Error::Infrastructure("VCP index record lacks generation vector".into())
            })?;
        index.insert(record, vector)?;
    }
    Ok(index)
}

impl VcpServingGeneration {
    /// Recompute request-local numerical assets from retained vectors and authorized support.
    /// This never invokes an embedding or generation provider and does not mutate stored assets.
    pub(crate) fn authorized(
        &self,
        allowed: &std::collections::HashSet<CognitiveRef>,
    ) -> Result<Self> {
        let mut documents = Vec::new();
        for (id, vector) in &self.assets.vectors {
            let reference = self.assets.identities.reference(*id)?;
            if !allowed.contains(reference) {
                continue;
            }
            let curve = self.assets.curves.iter().find(|c| c.id == *id);
            let concept_refs = curve
                .into_iter()
                .flat_map(|c| &c.tags)
                .filter_map(|tag| self.assets.identities.reference(tag.id).ok())
                .filter(|r| allowed.contains(*r))
                .cloned()
                .collect();
            documents.push(VcpProjectedDocument {
                reference: reference.clone(),
                representation_text: self
                    .assets
                    .labels
                    .iter()
                    .find(|v| v.0 == *id)
                    .map(|v| v.1.clone())
                    .unwrap_or_default(),
                vector: vector.clone(),
                concept_refs,
                curve_order: self
                    .assets
                    .curve_orders
                    .iter()
                    .find(|v| v.0 == *id)
                    .map(|v| v.1)
                    .unwrap_or(VcpCurveOrder::StableIdentity),
                evidence_roots: self
                    .assets
                    .candidate_evidence_roots
                    .iter()
                    .find(|v| v.0 == *id)
                    .map(|v| v.1.clone())
                    .unwrap_or_default(),
            });
        }
        let mut edges = Vec::new();
        for edge in &self.assets.graph.evidence {
            let from = self.assets.identities.reference(edge.source_id)?;
            let to = self.assets.identities.reference(edge.target_id)?;
            if !allowed.contains(from)
                || !allowed.contains(to)
                || edge.source_refs.is_empty()
                || !edge.source_refs.iter().all(|r| allowed.contains(r))
            {
                continue;
            }
            edges.push(WaveEdgeEvidence {
                source_refs: edge.source_refs.clone(),
                from: from.clone(),
                to: to.clone(),
                basis_class: edge.basis_class.clone(),
                association_kind: edge.association_kind.clone(),
                polarity: "positive".into(),
                support_mass: edge.support_mass,
                provenance_root: edge.provenance_root.clone(),
            });
        }
        let identities = VcpIdentityMap::new(
            documents
                .iter()
                .map(|d| d.reference.clone())
                .chain(edges.iter().flat_map(|e| [e.from.clone(), e.to.clone()])),
        );
        let assets = VcpGeneration::build(
            self.assets.generation_id,
            self.assets.cognitive_profile,
            VcpProjectionMaterial {
                authority_watermark: self.assets.authority_watermark,
                space: self.assets.space.clone(),
                producer: self.assets.producer.clone(),
                identities,
                edges,
                documents,
            },
            self.assets.policy.clone(),
        )?;
        let candidates = build_index(&assets, false)?;
        let tags = build_index(&assets, true)?;
        Ok(Self {
            assets,
            candidates,
            tags,
        })
    }
}
