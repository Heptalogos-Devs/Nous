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
    pub fn search_candidates(&self, vector: &[f32], limit: usize) -> Result<Vec<DenseMatch>> {
        if self.candidates.is_empty() {
            return Ok(Vec::new());
        }
        self.candidates.search(vector, limit)
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
                embedding_space: assets.space.clone(),
                producer_signature: assets.producer.signature_hash.clone(),
                representation_kind: if tags_only { "tag" } else { "vcp_candidate" }.into(),
                source_region: None,
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
