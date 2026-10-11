// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reference", rename_all = "snake_case")]
pub enum TextEmbeddingOrigin {
    Query,
    Document(CognitiveRef),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddingRequest {
    pub subject: SubjectId,
    pub text: String,
    pub origin: TextEmbeddingOrigin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEmbeddingOutput {
    pub vector: Vec<f32>,
    pub space: EmbeddingSpaceSignature,
    pub producer: ProducerSignature,
}

#[async_trait::async_trait]
pub trait TextEmbeddingProvider: Send + Sync {
    fn space(&self) -> EmbeddingSpaceSignature;
    fn producer(&self) -> ProducerSignature;
    fn producers(&self) -> Vec<ProducerSignature> {
        vec![self.producer()]
    }
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<TextEmbeddingOutput>;
}

pub(crate) struct RequestEmbedding {
    inner: std::sync::Arc<dyn TextEmbeddingProvider>,
    text: String,
    output: tokio::sync::OnceCell<TextEmbeddingOutput>,
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
    async fn embed(&self, request: TextEmbeddingRequest) -> Result<TextEmbeddingOutput> {
        if request.origin != TextEmbeddingOrigin::Query || request.text != self.text {
            return Err(Error::new(
                nous_core::ErrorOwner::Resource,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            ));
        }
        self.output
            .get_or_try_init(|| async {
                for producer in self.producers() {
                    if let Some(output) = crate::material::query_material_output(
                        &request.text,
                        &self.space(),
                        &producer,
                    ) {
                        return Ok(output);
                    }
                }
                self.inner.embed(request).await
            })
            .await
            .cloned()
    }
}
