//! Profile-independent Tag semantic vectors and one-step attachment postings.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use crate::{assets::files::*, *};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptRecord {
    pub tag: TagId,
    pub revision: uuid::Uuid,
    pub semantic: TagSemanticRepresentation,
    pub vector: Option<Vec<f32>>,
    pub vector_producer: Option<ProducerSignature>,
    pub attachments: Vec<CognitiveRef>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptGeneration {
    pub generation_id: ServingGenerationId,
    pub authority_watermark: i64,
    pub space: Option<EmbeddingSpaceSignature>,
    pub producer: Option<ProducerSignature>,
    pub records: Vec<ConceptRecord>,
}
impl ConceptGeneration {
    pub fn record(&self, tag: TagId) -> Option<&ConceptRecord> {
        self.records
            .binary_search_by_key(&tag.0, |r| r.tag.0)
            .ok()
            .map(|i| &self.records[i])
    }
    pub(crate) fn semantic_vector(&self, tag: TagId, text: &str) -> Option<Vec<f32>> {
        self.record(tag)
            .filter(|record| record.semantic.text == text)?
            .vector
            .clone()
    }
    pub fn validate(&self) -> Result<()> {
        if self.records.windows(2).any(|r| r[0].tag.0 >= r[1].tag.0)
            || self.space.is_some() != self.producer.is_some()
        {
            return Err(Error::Invalid(
                "invalid concept generation identities".into(),
            ));
        }
        for record in &self.records {
            if record.vector.is_some() != record.vector_producer.is_some() {
                return Err(Error::Invalid("concept vector producer missing".into()));
            }
            if record.semantic.version != TAG_REPRESENTATION_VERSION
                || record.semantic.digest
                    != blake3::hash(
                        format!("{}\n{}", record.semantic.version, record.semantic.text).as_bytes(),
                    )
                    .to_hex()
                    .as_str()
            {
                return Err(Error::Invalid("concept semantic digest mismatch".into()));
            }
            if let Some(vector) = &record.vector
                && (self
                    .space
                    .as_ref()
                    .is_none_or(|space| vector.len() != space.dimension as usize)
                    || vector.iter().any(|v| !v.is_finite()))
            {
                return Err(Error::Invalid("invalid concept vector".into()));
            }
        }
        Ok(())
    }
}
impl ServingService {
    pub(crate) async fn build_concept(
        &self,
        subject: SubjectId,
        id: ServingGenerationId,
        space_key: &str,
        dir: &std::path::Path,
        view: Option<&HistoricalAuthoritySnapshot>,
        capabilities: ProjectionCapabilities,
    ) -> Result<i64> {
        let input = self
            .store
            .concept_projection_input(subject, capabilities.memory, view)
            .await?;
        let provider = if space_key.is_empty() {
            None
        } else {
            self.embedding()
                .filter(|p| p.space().space_hash == space_key)
        };
        if !space_key.is_empty() && provider.is_none() {
            return Err(Error::Unavailable(
                "concept embedding space is unavailable".into(),
            ));
        }
        let cached = self.publisher.snapshot_for(subject);
        let space = provider.map(|p| p.space());
        let producer = provider.map(|p| p.producer());
        let mut records = Vec::new();
        for tag in input.tags {
            let reused = cached
                .concept
                .iter()
                .filter(|g| {
                    g.space.as_ref().map(|s| &s.space_hash) == space.as_ref().map(|s| &s.space_hash)
                        && g.producer.as_ref().map(|p| &p.signature_hash)
                            == producer.as_ref().map(|p| &p.signature_hash)
                })
                .find_map(|g| {
                    g.record(tag.tag)
                        .filter(|r| r.semantic.digest == tag.semantic.digest)
                        .and_then(|r| Some((r.vector.clone()?, r.vector_producer.clone()?)))
                });
            let (vector, vector_producer) = reused
                .map(|(vector, producer)| (Some(vector), Some(producer)))
                .unwrap_or_default();
            records.push(ConceptRecord {
                tag: tag.tag,
                revision: tag.revision,
                semantic: tag.semantic,
                vector,
                vector_producer,
                attachments: tag.attachments,
            });
        }
        if let Some(provider) = provider {
            let allowed = provider
                .producers()
                .into_iter()
                .map(|producer| producer.signature_hash)
                .collect::<std::collections::HashSet<_>>();
            let missing = records
                .iter()
                .enumerate()
                .filter_map(|(index, record)| record.vector.is_none().then_some(index))
                .collect::<Vec<_>>();
            for chunk in missing.chunks(256) {
                let texts = chunk
                    .iter()
                    .map(|index| records[*index].semantic.text.clone())
                    .collect::<Vec<_>>();
                let outputs = provider.embed_batch(subject, &texts).await?;
                if outputs.len() != chunk.len()
                    || outputs.iter().any(|output| {
                        !output
                            .space
                            .compatible_with(space.as_ref().expect("vector concept space"))
                            || !allowed.contains(&output.producer.signature_hash)
                    })
                {
                    return Err(Error::Conflict(
                        "concept embedding batch length/space/producer mismatch".into(),
                    ));
                }
                for (index, output) in chunk.iter().zip(outputs) {
                    records[*index].vector = Some(output.vector);
                    records[*index].vector_producer = Some(output.producer);
                }
            }
        }
        let generation = ConceptGeneration {
            generation_id: id,
            authority_watermark: input.watermark,
            space,
            producer,
            records,
        };
        generation.validate()?;
        write_json(&dir.join("concept.json"), &generation)?;
        Ok(input.watermark)
    }
}
