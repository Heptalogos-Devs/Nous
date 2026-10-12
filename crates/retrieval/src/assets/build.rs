// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::files::*, *};

#[derive(Serialize, Deserialize)]
pub(crate) struct DenseManifest {
    pub space: EmbeddingSpaceSignature,
    pub records: Vec<VectorRecord>,
}

pub(super) struct TextDocuments {
    watermarks: nous_persistence::ProjectionWatermarks,
    documents: std::sync::Arc<Vec<LexicalDocument>>,
}

impl ServingService {
    pub(super) async fn build_family_in_view(
        &self,
        subject: SubjectId,
        family_space: (&str, &str),
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
        capabilities: ProjectionCapabilities,
        text_documents: &mut Option<TextDocuments>,
    ) -> Result<(ServingRecord, super::lifecycle::OpenArtifact)> {
        let (family, space) = family_space;
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&self.options.root)
            .map_err(io)?;
        let id = ServingGenerationId::new();
        if matches!(family, "lexical" | "dense") && text_documents.is_none() {
            let budget = snapshot.get(crate::EPISODE_SYNOPSIS)?;
            let input = self
                .store
                .text_projection_input(subject, capabilities.memory, budget, view)
                .await?;
            *text_documents = Some(TextDocuments {
                watermarks: input.watermarks,
                documents: std::sync::Arc::new(
                    self.documents(subject, input.sources, budget, view).await?,
                ),
            });
        }
        let watermark = match super::family::AssetFamily::parse(family)? {
            super::family::AssetFamily::Topology => {
                self.build_topology(
                    subject,
                    id,
                    staging.path(),
                    snapshot,
                    capabilities,
                    view,
                    serving,
                )
                .await?
            }
            super::family::AssetFamily::Concept => {
                self.build_concept(subject, id, space, staging.path(), view, capabilities)
                    .await?
            }
            super::family::AssetFamily::Dense => {
                self.build_dense(
                    subject,
                    id,
                    space,
                    staging.path(),
                    serving,
                    text_documents
                        .as_ref()
                        .ok_or_else(|| Error::Infrastructure("text projection missing".into()))?,
                )
                .await?
            }
            super::family::AssetFamily::Lexical => {
                self.build_text(
                    staging.path(),
                    snapshot,
                    text_documents
                        .as_ref()
                        .ok_or_else(|| Error::Infrastructure("text projection missing".into()))?,
                )
                .await?
            }
        };
        let sums = checksums(staging.path())?;
        let hash = digest(&sums)?;
        let target = self.artifact_directory(id);
        std::fs::rename(staging.path(), &target).map_err(io)?;
        // Open the final physical artifact once before publishing its pointer.
        let artifact = self.readback(&target, family, id)?;
        let config_digest = self.config_digest(family, space, snapshot, capabilities)?;
        let record = ServingRecord {
            generation_id: id,
            subject,
            family: family.into(),
            view: view.map_or(nous_persistence::ServingView::Current, |v| {
                nous_persistence::ServingView::Historical {
                    snapshot_digest: v.snapshot_digest.clone(),
                    as_of: v.as_of,
                    revision_view: v.revision_view,
                }
            }),
            space: space.into(),
            authority_watermark: watermark,
            config_digest: config_digest.clone(),
            artifact_hash: hash,
            built_at: chrono::Utc::now(),
            metadata: serde_json::json!({}),
        };
        let published = self.store.publish_generation(record).await?;
        if published.generation_id == id {
            return Ok((published, artifact));
        }
        // Native handles must close before deleting the unselected directory.
        drop(artifact);
        std::fs::remove_dir_all(&target).map_err(io)?;
        let selected = self.open_record(&published)?;
        Ok((published, selected))
    }

    async fn build_text(
        &self,
        dir: &std::path::Path,
        snapshot: &nous_configuration::ConfigSnapshot,
        input: &TextDocuments,
    ) -> Result<i64> {
        let writer_bytes = snapshot.get(crate::LEXICAL_WRITER_BYTES)?;
        let documents = input.documents.clone();
        let directory = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut lexical = LexicalGeneration::create(directory)?;
            lexical.add_documents(&documents, writer_bytes)
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))??;
        Ok(input.watermarks.desired("lexical", ""))
    }

    async fn build_topology(
        &self,
        subject: SubjectId,
        id: ServingGenerationId,
        dir: &std::path::Path,
        snapshot: &nous_configuration::ConfigSnapshot,
        capabilities: ProjectionCapabilities,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
    ) -> Result<i64> {
        let profile = snapshot.get(nous_runtime::COGNITIVE_PROFILE)?;
        if matches!(
            profile,
            nous_runtime::CognitiveProfile::VcpDtsc | nous_runtime::CognitiveProfile::VcpRiverMemo
        ) {
            let material = self
                .vcp_projection_material_in_view(subject, snapshot, view, serving, capabilities)
                .await?;
            let watermark = material.authority_watermark;
            let policy = snapshot.get(crate::VCP_ASSETS)?;
            let directory = dir.to_path_buf();
            let path = dir.join("vcp.json");
            tokio::task::spawn_blocking(move || {
                let assets = VcpGeneration::build(id, profile, material, policy)?;
                let generation = VcpServingGeneration::create(assets, &directory)?;
                write_json(&path, &generation)
            })
            .await
            .map_err(|e| Error::Infrastructure(e.to_string()))??;
            return Ok(watermark);
        }
        let input = self
            .topology_projection(subject, capabilities.memory, view)
            .await?;
        let edges = input.edges;
        let nodes = input
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, reference)| WaveNode {
                serving_id: index as u32,
                reference,
            })
            .collect();
        let path = dir.join("topology.json");
        let config = resolve_wave_config(snapshot)?;
        let cognitive_profile = nous_runtime::CognitiveProfile::NousNodePotential;
        tokio::task::spawn_blocking(move || {
            let mut graph = WaveGraphGeneration::build(nodes, &edges, config)?;
            graph.generation_id = id;
            graph.cognitive_profile = cognitive_profile;
            write_json(&path, &graph.artifact())
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))??;
        Ok(input.watermark)
    }

    pub(crate) async fn embed_documents(
        &self,
        subject: SubjectId,
        documents: &[LexicalDocument],
        concepts: Option<&ConceptGeneration>,
        provider: &dyn TextEmbeddingProvider,
    ) -> Result<Vec<(usize, QuerySemanticEmbedding)>> {
        let space = provider.space();
        let producers: std::collections::HashSet<_> = provider
            .producers()
            .into_iter()
            .map(|producer| producer.signature_hash)
            .collect();
        let texts: Vec<_> = documents
            .iter()
            .filter(|document| !matches!(document.reference, CognitiveRef::Tag(_)))
            .map(|document| document.representation_text.clone())
            .collect();
        let mut ordinary = Vec::with_capacity(texts.len());
        for batch in texts.chunks(256) {
            let outputs = provider.embed_batch(subject, batch).await?;
            if outputs.len() != batch.len() {
                return Err(Error::Invalid(
                    "embedding batch result count mismatch".into(),
                ));
            }
            ordinary.extend(outputs);
        }
        let mut ordinary = ordinary.into_iter();
        let mut projected = Vec::with_capacity(documents.len());
        for (index, document) in documents.iter().enumerate() {
            let output = if let CognitiveRef::Tag(tag) = document.reference {
                let Some(vector) = concepts.and_then(|generation| {
                    generation.semantic_vector(tag, &document.representation_text)
                }) else {
                    continue;
                };
                QuerySemanticEmbedding {
                    vector,
                    space: space.clone(),
                    producer: concepts
                        .and_then(|generation| generation.record(tag))
                        .and_then(|record| record.vector_producer.clone())
                        .ok_or_else(|| Error::Invalid("concept vector producer missing".into()))?,
                }
            } else {
                ordinary
                    .next()
                    .ok_or_else(|| Error::Infrastructure("embedding batch result missing".into()))?
            };
            if !output.space.compatible_with(&space)
                || !producers.contains(&output.producer.signature_hash)
            {
                return Err(Error::Conflict(
                    "projection embedding disagrees with configured space/producer".into(),
                ));
            }
            if output.vector.len() != space.dimension as usize
                || output.vector.iter().any(|value| !value.is_finite())
            {
                return Err(Error::Invalid(
                    "projection embedding has invalid dimension or values".into(),
                ));
            }
            projected.push((index, output));
        }
        Ok(projected)
    }
    async fn build_dense(
        &self,
        subject: SubjectId,
        id: ServingGenerationId,
        space_key: &str,
        dir: &std::path::Path,
        serving: &ServingSnapshot,
        input: &TextDocuments,
    ) -> Result<i64> {
        let provider = self
            .embedding()
            .ok_or_else(|| Error::Unavailable("text embedding provider is unavailable".into()))?;
        let space = provider.space();
        if space_key != space.space_hash {
            return Err(Error::Unavailable(
                "embedding space has no configured provider".into(),
            ));
        }
        let documents = &input.documents;
        let concepts = serving.concept.iter().find(|generation| {
            generation
                .space
                .as_ref()
                .is_some_and(|s| s.compatible_with(&space))
                && generation
                    .producer
                    .as_ref()
                    .is_some_and(|p| p.signature_hash == provider.producer().signature_hash)
        });
        let mut vectors = Vec::new();
        for (index, output) in self
            .embed_documents(
                subject,
                documents,
                concepts.map(AsRef::as_ref),
                provider.as_ref(),
            )
            .await?
        {
            let document = &documents[index];
            let record = VectorRecord {
                serving_doc_id: document.serving_doc_id,
                reference: document.reference.clone(),
                producer_signature: output.producer.signature_hash,
            };
            vectors.push((record, output.vector));
        }
        let directory = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut generation = DenseGeneration::new(space.clone(), vectors.len())?;
            generation.generation_id = id;
            let mut records = Vec::new();
            for (record, vector) in vectors {
                generation.insert(record.clone(), &vector)?;
                records.push(record);
            }
            generation.save(&directory.join("vectors.usearch"))?;
            write_json(
                &directory.join("records.json"),
                &DenseManifest { space, records },
            )
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))??;
        Ok(input.watermarks.desired("dense", space_key))
    }
}
