// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{CognitiveRef, EmbeddingSpaceSignature, Error, Result, ServingGenerationId};
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use usearch::{
    Index,
    ffi::{IndexOptions, MetricKind, ScalarKind},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorRecord {
    pub serving_doc_id: u64,
    pub reference: CognitiveRef,
    pub embedding_space: EmbeddingSpaceSignature,
    pub producer_signature: String,
    pub representation_kind: String,
    pub source_region: Option<CognitiveRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenseMatch {
    pub serving_doc_id: u64,
    pub distance: f32,
    pub similarity: f32,
    pub record: Option<VectorRecord>,
}

pub struct DenseGeneration {
    pub generation_id: ServingGenerationId,
    pub space: EmbeddingSpaceSignature,
    index: Arc<Index>,
    records: HashMap<u64, VectorRecord>,
    vectors: HashMap<u64, Vec<f32>>,
}

impl DenseGeneration {
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.index
            .save(
                path.to_str()
                    .ok_or_else(|| Error::Invalid("index path must be UTF-8".into()))?,
            )
            .map_err(|error| Error::Infrastructure(format!("USearch save: {error}")))
    }

    pub fn open(
        path: &std::path::Path,
        generation_id: ServingGenerationId,
        space: EmbeddingSpaceSignature,
        records: Vec<VectorRecord>,
    ) -> Result<Self> {
        let metric = metric_for_space(&space);
        Self::open_with_metric(path, generation_id, space, records, metric)
    }

    pub(crate) fn open_with_metric(
        path: &std::path::Path,
        generation_id: ServingGenerationId,
        space: EmbeddingSpaceSignature,
        records: Vec<VectorRecord>,
        metric: MetricKind,
    ) -> Result<Self> {
        let mut generation = Self::new_with_metric(space, records.len(), metric)?;
        generation
            .index
            .load(
                path.to_str()
                    .ok_or_else(|| Error::Invalid("index path must be UTF-8".into()))?,
            )
            .map_err(|error| Error::Infrastructure(format!("USearch load: {error}")))?;
        for record in records {
            if !record.embedding_space.compatible_with(&generation.space) {
                return Err(Error::Conflict(
                    "dense artifact contains an incompatible embedding space".into(),
                ));
            }
            let mut vector = vec![0.0_f32; generation.space.dimension as usize];
            let count = generation
                .index
                .get(record.serving_doc_id, &mut vector)
                .map_err(|error| Error::Infrastructure(format!("USearch readback: {error}")))?;
            if count != 1 {
                return Err(Error::Infrastructure(
                    "dense record is missing from index".into(),
                ));
            }
            validate_vector(&generation.space, &vector)?;
            generation.vectors.insert(record.serving_doc_id, vector);
            generation.records.insert(record.serving_doc_id, record);
        }
        if generation.index.size() != generation.records.len() {
            return Err(Error::Infrastructure(
                "dense index and record manifest disagree".into(),
            ));
        }
        generation.generation_id = generation_id;
        Ok(generation)
    }
    pub fn new(space: EmbeddingSpaceSignature, capacity: usize) -> Result<Self> {
        let metric = metric_for_space(&space);
        Self::new_with_metric(space, capacity, metric)
    }

    pub(crate) fn new_with_metric(
        space: EmbeddingSpaceSignature,
        capacity: usize,
        metric: MetricKind,
    ) -> Result<Self> {
        if space.dimension == 0 {
            return Err(Error::Invalid(
                "embedding dimension must be positive".into(),
            ));
        }
        let options = IndexOptions {
            dimensions: space.dimension as usize,
            metric,
            quantization: ScalarKind::F32,
            connectivity: 16,
            expansion_add: 40,
            expansion_search: 32,
            multi: false,
        };
        let index = Index::new(&options)
            .map_err(|error| Error::Infrastructure(format!("USearch initialization: {error}")))?;
        index
            .reserve(capacity.max(1))
            .map_err(|error| Error::Infrastructure(format!("USearch reserve: {error}")))?;
        Ok(Self {
            generation_id: ServingGenerationId::new(),
            space,
            index: Arc::new(index),
            records: HashMap::new(),
            vectors: HashMap::new(),
        })
    }

    pub fn insert(&mut self, record: VectorRecord, vector: &[f32]) -> Result<()> {
        validate_vector(&self.space, vector)?;
        if !record.embedding_space.compatible_with(&self.space) {
            return Err(Error::Conflict(
                "vector embedding space does not match DenseGeneration".into(),
            ));
        }
        let serving_doc_id = record.serving_doc_id;
        self.index
            .add(record.serving_doc_id, vector)
            .map_err(|error| Error::Infrastructure(format!("USearch add: {error}")))?;
        self.records.insert(serving_doc_id, record);
        self.vectors.insert(serving_doc_id, vector.to_vec());
        Ok(())
    }

    pub fn search(&self, vector: &[f32], limit: usize) -> Result<Vec<DenseMatch>> {
        validate_vector(&self.space, vector)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let matches = self
            .index
            .search(vector, limit)
            .map_err(|error| Error::Infrastructure(format!("USearch search: {error}")))?;
        Ok(matches
            .keys
            .into_iter()
            .zip(matches.distances)
            .map(|(serving_doc_id, distance)| DenseMatch {
                serving_doc_id,
                distance,
                similarity: 1.0 / (1.0 + distance),
                record: self.records.get(&serving_doc_id).cloned(),
            })
            .collect())
    }

    pub fn search_filtered(
        &self,
        vector: &[f32],
        limit: usize,
        allowed: &RoaringBitmap,
    ) -> Result<Vec<DenseMatch>> {
        validate_vector(&self.space, vector)?;
        if limit == 0 || allowed.is_empty() {
            return Ok(vec![]);
        }
        let matches = self
            .index
            .filtered_search(vector, limit, |key| {
                key <= u64::from(u32::MAX) && allowed.contains(key as u32)
            })
            .map_err(|error| Error::Infrastructure(format!("USearch filtered search: {error}")))?;
        Ok(matches
            .keys
            .into_iter()
            .zip(matches.distances)
            .map(|(serving_doc_id, distance)| DenseMatch {
                serving_doc_id,
                distance,
                similarity: 1.0 / (1.0 + distance),
                record: self.records.get(&serving_doc_id).cloned(),
            })
            .collect())
    }

    pub fn record(&self, serving_doc_id: u64) -> Option<&VectorRecord> {
        self.records.get(&serving_doc_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn records(&self) -> impl Iterator<Item = &VectorRecord> {
        self.records.values()
    }

    pub fn vector(&self, serving_doc_id: u64) -> Option<&[f32]> {
        self.vectors.get(&serving_doc_id).map(Vec::as_slice)
    }
}

fn validate_vector(space: &EmbeddingSpaceSignature, vector: &[f32]) -> Result<()> {
    if vector.len() != space.dimension as usize || vector.iter().any(|value| !value.is_finite()) {
        return Err(Error::Invalid(
            "embedding vector dimension or values are invalid".into(),
        ));
    }
    if space.normalization.eq_ignore_ascii_case("l2") {
        let norm = vector
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt();
        if norm <= f64::EPSILON {
            return Err(Error::Invalid("normalized embedding vector is zero".into()));
        }
    }
    Ok(())
}

fn metric_for_space(space: &EmbeddingSpaceSignature) -> MetricKind {
    if space.normalization.eq_ignore_ascii_case("l2") {
        MetricKind::Cos
    } else {
        MetricKind::L2sq
    }
}
