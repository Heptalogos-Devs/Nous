// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::Result;
use nous_retrieval::{QueryEmbedding, TextEmbeddingOutput};

impl KernelService {
    pub(super) fn embedding_config(&self) -> Result<k::EmbeddingConfig> {
        let provider = self.0.serving.embedding().ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Core,
                nous_core::ErrorCode::Unavailable,
                "embedding space not configured",
            )
        })?;
        let space = provider.space();
        Ok(k::EmbeddingConfig {
            space_hash: space.space_hash,
            producer_hash: provider.producer().signature_hash,
            model: space.model_identity,
            dimension: space.dimension,
            producer_hashes: provider
                .producers()
                .iter()
                .map(|p| p.signature_hash.clone())
                .collect(),
        })
    }
    pub(super) fn query_materials(
        &self,
        bound: &nous_runtime::BoundQuery,
        inputs: Vec<k::QueryEmbedding>,
    ) -> Result<Vec<QueryEmbedding>> {
        if inputs.len() > 1 {
            return Err(Error::new(
                nous_core::ErrorOwner::Core,
                nous_core::ErrorCode::InvalidRequest,
                "Query accepts at most one supplied embedding",
            ));
        }
        let mut materials = vec![];
        for material in inputs {
            let provider = self.0.serving.embedding().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::Unavailable,
                    "embedding space not configured",
                )
            })?;
            let space = provider.space();
            let producer = provider
                .producers()
                .into_iter()
                .find(|p| p.signature_hash == material.producer_hash)
                .ok_or_else(|| {
                    Error::new(
                        nous_core::ErrorOwner::Core,
                        nous_core::ErrorCode::InvalidRequest,
                        "query embedding producer is not authorized",
                    )
                })?;
            if material.text != bound.representation.text {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding text does not match the prepared query representation",
                ));
            }
            if material.text.len() > 131072 {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding text exceeds the 131,072-byte limit",
                ));
            }
            if material.space_hash != space.space_hash {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding space does not match the configured space",
                ));
            }
            if material.producer_hash != producer.signature_hash {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding producer is not authorized",
                ));
            }
            if material.vector.len() != space.dimension as usize {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding vector dimension does not match the configured space",
                ));
            }
            if material.vector.iter().any(|v| !v.is_finite()) {
                return Err(Error::new(
                    nous_core::ErrorOwner::Core,
                    nous_core::ErrorCode::InvalidRequest,
                    "Query embedding vector contains a non-finite value",
                ));
            }
            materials.push(QueryEmbedding {
                text: material.text,
                output: TextEmbeddingOutput {
                    vector: material.vector,
                    space,
                    producer,
                },
            });
        }
        Ok(materials)
    }
}
