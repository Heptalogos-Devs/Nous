//! Host-supplied model material. This adapter never invokes a model or network provider.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredEmbeddingConfig {
    pub space: EmbeddingSpaceSignature,
    pub producers: Vec<ProducerSignature>,
}
pub struct StoredEmbeddingProvider {
    store: AuthorityStore,
    config: StoredEmbeddingConfig,
}
impl StoredEmbeddingProvider {
    pub fn new(store: AuthorityStore, config: StoredEmbeddingConfig) -> Result<Self> {
        validate_embedding_config(&config)?;
        Ok(Self { store, config })
    }
}
pub fn validate_embedding_config(config: &StoredEmbeddingConfig) -> Result<()> {
    let mut space = config.space.clone();
    space.space_hash.clear();
    let expected = blake3::hash(
        &serde_json::to_vec(&space).map_err(|error| Error::Invalid(error.to_string()))?,
    )
    .to_hex()
    .to_string();
    if config.space.space_hash != expected
        || config.space.dimension == 0
        || config.space.dimension > 8192
        || config.producers.is_empty()
        || config.producers.len() > 4
    {
        return Err(Error::Invalid(
            "invalid canonical embedding configuration".into(),
        ));
    }
    let mut hashes = std::collections::HashSet::new();
    for producer in &config.producers {
        if producer.signature_hash != AuthorityStore::canonical_producer(producer)?.signature_hash
            || producer.model_identity.as_deref() != Some(config.space.model_identity.as_str())
            || producer.operation != CapabilityOperation::TextEmbedding
            || !hashes.insert(&producer.signature_hash)
        {
            return Err(Error::Invalid(
                "invalid canonical embedding producer".into(),
            ));
        }
    }
    Ok(())
}
#[async_trait::async_trait]
impl TextEmbeddingProvider for StoredEmbeddingProvider {
    fn space(&self) -> EmbeddingSpaceSignature {
        self.config.space.clone()
    }
    fn producer(&self) -> ProducerSignature {
        self.config.producers[0].clone()
    }
    fn producers(&self) -> Vec<ProducerSignature> {
        self.config.producers.clone()
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<QuerySemanticEmbedding> {
        for producer in &self.config.producers {
            if !request.query {
                let vector = sqlx::query_scalar::<_,Vec<f32>>("SELECT vector FROM embedding_materials WHERE subject_id=$1 AND content_digest=$2 AND space_hash=$3 AND producer_hash=$4")
                    .bind(request.subject.0).bind(nous_material::text_content_identity(&request.text)).bind(&self.config.space.space_hash).bind(&producer.signature_hash)
                    .fetch_optional(self.store.pool()).await.map_err(db)?;
                if let Some(vector) = vector {
                    return Ok(QuerySemanticEmbedding {
                        vector,
                        space: self.space(),
                        producer: producer.clone(),
                    });
                }
            }
        }
        Err(Error::Unavailable(
            "Host did not supply compatible embedding material".into(),
        ))
    }
}
