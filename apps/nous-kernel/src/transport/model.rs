// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::Result;
use nous_retrieval::{QueryEmbedding, TextEmbeddingOutput};

impl KernelService {
    pub(super) fn embedding_config(&self) -> Result<k::EmbeddingConfig> {
        let provider = self.0.serving.embedding().ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::ModelWorkflow,
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
                nous_core::ErrorOwner::ModelWorkflow,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        let mut materials = vec![];
        for material in inputs {
            let provider = self.0.serving.embedding().ok_or_else(|| {
                Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
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
                        nous_core::ErrorOwner::ModelWorkflow,
                        nous_core::ErrorCode::InvalidRequest,
                        "query embedding producer is not authorized",
                    )
                })?;
            if material.text != bound.representation.text
                || material.text.len() > 131072
                || material.space_hash != space.space_hash
                || material.producer_hash != producer.signature_hash
                || material.vector.len() != space.dimension as usize
                || material.vector.iter().any(|v| !v.is_finite())
            {
                return Err(Error::new(
                    nous_core::ErrorOwner::ModelWorkflow,
                    nous_core::ErrorCode::InvalidRequest,
                    "Invalid request",
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

pub(super) fn workflow_json(value: &str) -> Result<serde_json::Value> {
    if value.len() > nous_persistence::WORKFLOW_VALUE_MAX_BYTES {
        return Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "workflow value exceeds bound",
        ));
    }
    serde_json::from_str(value).map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "workflow JSON is invalid",
        )
        .with_source(error)
    })
}

pub(super) fn private_workflow_owner(owner: &str) -> Result<()> {
    nous_persistence::WorkflowOwner::new(owner)?;
    if ["memory", "material"].contains(&owner) {
        Ok(())
    } else {
        Err(Error::new(
            nous_core::ErrorOwner::ModelWorkflow,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        ))
    }
}
