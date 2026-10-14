//! Host-supplied model material. This adapter never invokes a model or network provider.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;
use nous_persistence::database_error as db;
use sqlx::Row;

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
        if request.query {
            return Err(Error::Unavailable(
                "Host did not supply a query embedding".into(),
            ));
        }
        self.embed_batch(request.subject, &[request.text])
            .await?
            .pop()
            .ok_or_else(|| {
                Error::Unavailable("Host did not supply compatible embedding material".into())
            })
    }
    async fn embed_batch(
        &self,
        subject: SubjectId,
        texts: &[String],
    ) -> Result<Vec<QuerySemanticEmbedding>> {
        if texts.len() > 256 {
            return Err(Error::Invalid(
                "embedding material batch exceeds 256".into(),
            ));
        }
        let digests: Vec<_> = texts
            .iter()
            .map(|text| nous_material::text_content_identity(text))
            .collect();
        let producers: Vec<_> = self
            .config
            .producers
            .iter()
            .map(|producer| producer.signature_hash.clone())
            .collect();
        let rows = sqlx::query(r#"
SELECT cached.vector,cached.producer_hash FROM unnest($2::text[]) WITH ORDINALITY needed(digest,ordinal)
JOIN LATERAL (SELECT vector,producer_hash FROM embedding_materials WHERE subject_id=$1 AND content_digest=needed.digest AND space_hash=$3 AND producer_hash=ANY($4::text[]) ORDER BY array_position($4::text[],producer_hash) LIMIT 1) cached ON true ORDER BY needed.ordinal
"#).bind(subject.0).bind(digests).bind(&self.config.space.space_hash).bind(producers).fetch_all(self.store.pool()).await.map_err(db)?;
        if rows.len() != texts.len() {
            return Err(Error::Unavailable(
                "Host did not supply compatible embedding material".into(),
            ));
        }
        rows.into_iter()
            .map(|row| {
                let hash: String = row.try_get("producer_hash").map_err(db)?;
                let producer = self
                    .config
                    .producers
                    .iter()
                    .find(|producer| producer.signature_hash == hash)
                    .ok_or_else(|| {
                        Error::Infrastructure("cached embedding producer not configured".into())
                    })?;
                Ok(QuerySemanticEmbedding {
                    vector: row.try_get("vector").map_err(db)?,
                    space: self.space(),
                    producer: producer.clone(),
                })
            })
            .collect()
    }
}
