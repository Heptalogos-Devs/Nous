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
}

impl DenseGeneration {
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        self.index
            .save(path.to_str().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "index path must be UTF-8",
                )
            })?)
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not save the dense Serving index",
                )
                .with_source(error)
            })
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
            .load(path.to_str().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InvalidRequest,
                    "index path must be UTF-8",
                )
            })?)
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not open the dense Serving index",
                )
                .with_source(error)
            })?;
        let mut vector = vec![0.0_f32; generation.space.dimension as usize];
        for record in records {
            if !record.embedding_space.compatible_with(&generation.space) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::Conflict,
                    "Conflict",
                ));
            }
            read_recorded_vector(&generation.index, record.serving_doc_id, &mut vector)?;
            validate_vector(&generation.space, &vector)?;
            generation.records.insert(record.serving_doc_id, record);
        }
        if generation.index.size() != generation.records.len() {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Dense Serving index size does not match its records",
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
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
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
        let index = Index::new(&options).map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not create the dense Serving index",
            )
            .with_source(error)
        })?;
        index.reserve(capacity.max(1)).map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not reserve capacity in the dense Serving index",
            )
            .with_source(error)
        })?;
        Ok(Self {
            generation_id: ServingGenerationId::new(),
            space,
            index: Arc::new(index),
            records: HashMap::new(),
        })
    }

    pub fn insert(&mut self, record: VectorRecord, vector: &[f32]) -> Result<()> {
        validate_vector(&self.space, vector)?;
        if !record.embedding_space.compatible_with(&self.space) {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::Conflict,
                "Conflict",
            ));
        }
        let serving_doc_id = record.serving_doc_id;
        self.index
            .add(record.serving_doc_id, vector)
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not add a vector to the dense Serving index",
                )
                .with_source(error)
            })?;
        self.records.insert(serving_doc_id, record);
        Ok(())
    }

    pub fn search(&self, vector: &[f32], limit: usize) -> Result<Vec<DenseMatch>> {
        validate_vector(&self.space, vector)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let matches = self.index.search(vector, limit).map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InternalFailure,
                "Could not search the dense Serving index",
            )
            .with_source(error)
        })?;
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
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Resource,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not search the filtered dense Serving index",
                )
                .with_source(error)
            })?;
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

    pub fn vector(&self, serving_doc_id: u64) -> Result<Option<Vec<f32>>> {
        if !self.records.contains_key(&serving_doc_id) {
            return Ok(None);
        }

        let mut vector = vec![0.0_f32; self.space.dimension as usize];
        read_recorded_vector(&self.index, serving_doc_id, &mut vector)?;
        Ok(Some(vector))
    }
}

fn read_recorded_vector(index: &Index, serving_doc_id: u64, vector: &mut [f32]) -> Result<()> {
    let count = index.get(serving_doc_id, vector).map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Resource,
            nous_core::ErrorCode::InternalFailure,
            "Could not read a vector from the dense Serving index",
        )
        .with_source(error)
    })?;
    if count != 1 {
        return Err(Error::new(
            nous_core::ErrorOwner::Resource,
            nous_core::ErrorCode::InternalFailure,
            "Dense Serving index is missing a recorded vector",
        ));
    }
    Ok(())
}

fn validate_vector(space: &EmbeddingSpaceSignature, vector: &[f32]) -> Result<()> {
    if vector.len() != space.dimension as usize || vector.iter().any(|value| !value.is_finite()) {
        return Err(Error::new(
            nous_core::ErrorOwner::Resource,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        ));
    }
    if space.normalization.eq_ignore_ascii_case("l2") {
        let norm = vector
            .iter()
            .map(|value| f64::from(*value) * f64::from(*value))
            .sum::<f64>()
            .sqrt();
        if norm <= f64::EPSILON {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "normalized embedding vector is zero",
            ));
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
