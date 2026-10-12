// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddingRequest {
    pub subject: SubjectId,
    pub text: String,
    pub query: bool,
}

use nous_runtime::QuerySemanticEmbedding;

#[async_trait::async_trait]
pub trait TextEmbeddingProvider: Send + Sync {
    fn space(&self) -> EmbeddingSpaceSignature;
    fn producer(&self) -> ProducerSignature;
    fn producers(&self) -> Vec<ProducerSignature> {
        vec![self.producer()]
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<QuerySemanticEmbedding>;
}

pub(crate) struct RequestEmbedding {
    inner: std::sync::Arc<dyn TextEmbeddingProvider>,
    text: String,
    output: tokio::sync::OnceCell<std::result::Result<QuerySemanticEmbedding, String>>,
}
impl RequestEmbedding {
    pub(crate) fn new(inner: std::sync::Arc<dyn TextEmbeddingProvider>, text: String) -> Self {
        Self {
            inner,
            text,
            output: Default::default(),
        }
    }
}
#[async_trait::async_trait]
impl TextEmbeddingProvider for RequestEmbedding {
    fn space(&self) -> EmbeddingSpaceSignature {
        self.inner.space()
    }
    fn producer(&self) -> ProducerSignature {
        self.inner.producer()
    }
    fn producers(&self) -> Vec<ProducerSignature> {
        self.inner.producers()
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<QuerySemanticEmbedding> {
        if !request.query || request.text != self.text {
            return Err(Error::Invalid(
                "embedding disagrees with immutable query representation".into(),
            ));
        }
        self.output
            .get_or_init(|| async {
                self.inner
                    .embed(request)
                    .await
                    .map_err(|error| error.to_string())
            })
            .await
            .clone()
            .map_err(Error::Unavailable)
    }
}
