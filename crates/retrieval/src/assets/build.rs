// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::{assets::files::*, *};

#[derive(Serialize, Deserialize)]
pub(crate) struct DenseManifest {
    pub space: EmbeddingSpaceSignature,
    pub records: Vec<VectorRecord>,
    pub basis: Option<EpaBasisGeneration>,
}

impl ServingService {
    pub(crate) async fn build_family(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<ServingRecord> {
        self.build_family_in_view(
            subject,
            family,
            space,
            snapshot,
            None,
            &self.publisher.snapshot_for(subject),
        )
        .await
    }
    pub(crate) async fn build_family_in_view(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
    ) -> Result<ServingRecord> {
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&self.options.root)
            .map_err(io)?;
        let id = ServingGenerationId::new();
        let capabilities = self.projection_capabilities(subject).await?;
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
                self.build_concept(subject, id, space, staging.path(), view)
                    .await?
            }
            super::family::AssetFamily::Dense => {
                self.build_dense(subject, id, space, staging.path(), snapshot, view, serving)
                    .await?
            }
            super::family::AssetFamily::Lexical => {
                self.build_text(
                    subject,
                    family,
                    staging.path(),
                    capabilities,
                    snapshot,
                    view,
                )
                .await?
            }
        };
        // Reopen the staged native artifact before publishing any durable pointer.
        self.readback(staging.path(), family, id)?;
        let sums = checksums(staging.path())?;
        let hash = digest(&sums)?;
        let target = self.options.root.join(id.0.to_string());
        std::fs::rename(staging.path(), &target).map_err(io)?;
        let implementation_id = implementation(family).to_owned();
        let config_digest = self.config_digest(subject, family, snapshot).await?;
        let implementation_revision = env!("NOUS_SERVING_IMPLEMENTATION_DIGEST");
        let cognitive_profile = (family == "topology")
            .then(|| topology_asset_profile(snapshot))
            .transpose()?;
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
            implementation_id: implementation_id.clone(),
            implementation_revision: implementation_revision.to_string(),
            config_digest: config_digest.clone(),
            artifact_location: target.to_string_lossy().into_owned(),
            artifact_hash: hash,
            built_at: chrono::Utc::now(),
            metadata: serde_json::json!({ "checksums": sums, "cognitive_profile": cognitive_profile }),
        };
        let published = self.store.publish_generation(record).await?;
        if published.generation_id != id {
            std::fs::remove_dir_all(&target).map_err(io)?;
        }
        Ok(published)
    }

    async fn build_text(
        &self,
        subject: SubjectId,
        family: &str,
        dir: &std::path::Path,
        capabilities: ProjectionCapabilities,
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<i64> {
        let writer_bytes = snapshot.get(crate::LEXICAL_WRITER_BYTES)?;
        let budget = snapshot.get(crate::EPISODE_SYNOPSIS)?;
        let input = match view {
            Some(view) => {
                let input = self.store.historical_projection_input(view, budget).await?;
                nous_persistence::TextProjectionInput {
                    watermark: 0,
                    sources: input.sources,
                }
            }
            None => {
                self.store
                    .text_projection_input(subject, family, "", capabilities.memory, budget)
                    .await?
            }
        };
        let documents = self.documents(subject, input.sources, budget, view).await?;
        let directory = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut lexical = LexicalGeneration::create(directory)?;
            lexical.add_documents(&documents, writer_bytes)
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))??;
        Ok(input.watermark)
    }

    async fn text_input_in_view(
        &self,
        subject: SubjectId,
        family: &str,
        space: &str,
        memory: bool,
        budget: nous_persistence::EpisodeTextBudget,
        view: Option<&HistoricalAuthoritySnapshot>,
    ) -> Result<nous_persistence::TextProjectionInput> {
        match view {
            Some(view) => {
                let input = self.store.historical_projection_input(view, budget).await?;
                Ok(nous_persistence::TextProjectionInput {
                    watermark: 0,
                    sources: input.sources,
                })
            }
            None => {
                self.store
                    .text_projection_input(subject, family, space, memory, budget)
                    .await
            }
        }
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
                .vcp_projection_material_in_view(subject, snapshot, view, serving)
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
            .projection_input(
                subject,
                capabilities.memory,
                snapshot.get(crate::EPISODE_SYNOPSIS)?,
                view,
            )
            .await?;
        let edges = input.topology.edges;
        let nodes = input
            .topology
            .nodes
            .into_iter()
            .enumerate()
            .map(|(index, reference)| {
                let node_kind = match reference {
                    CognitiveRef::Tag(_) => WaveNodeKind::Tag,
                    CognitiveRef::CognitiveSchema(_) | CognitiveRef::CognitiveSchemaRevision(_) => {
                        WaveNodeKind::Schema
                    }
                    CognitiveRef::Entity(_) => WaveNodeKind::Entity,
                    CognitiveRef::Resource(_) => WaveNodeKind::Resource,
                    _ => WaveNodeKind::Memory,
                };
                WaveNode {
                    serving_id: index as u32,
                    reference,
                    node_kind,
                    embedding_key: None,
                    posting_key: None,
                    intrinsic_residual_gain: None,
                }
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
        Ok(input.authority_watermark)
    }

    pub(crate) async fn embed_document(
        &self,
        subject: SubjectId,
        document: &LexicalDocument,
        concepts: Option<&ConceptGeneration>,
        provider: &dyn TextEmbeddingProvider,
    ) -> Result<Option<TextEmbeddingOutput>> {
        let output = if let CognitiveRef::Tag(tag) = document.reference {
            let Some(vector) = concepts.and_then(|generation| {
                generation.semantic_vector(tag, &document.representation_text)
            }) else {
                return Ok(None);
            };
            TextEmbeddingOutput {
                vector,
                space: provider.space(),
                producer: concepts
                    .and_then(|generation| generation.record(tag))
                    .and_then(|record| record.vector_producer.clone())
                    .ok_or_else(|| Error::Invalid("concept vector producer missing".into()))?,
            }
        } else {
            provider
                .embed(TextEmbeddingRequest {
                    subject,
                    text: document.representation_text.clone(),
                    query: false,
                })
                .await?
        };
        if !output.space.compatible_with(&provider.space())
            || !provider
                .producers()
                .iter()
                .any(|p| p.signature_hash == output.producer.signature_hash)
        {
            return Err(Error::Conflict(
                "projection embedding disagrees with configured space/producer".into(),
            ));
        }
        if output.vector.len() != provider.space().dimension as usize
            || output.vector.iter().any(|value| !value.is_finite())
        {
            return Err(Error::Invalid(
                "projection embedding has invalid dimension or values".into(),
            ));
        }
        Ok(Some(output))
    }
    async fn build_dense(
        &self,
        subject: SubjectId,
        id: ServingGenerationId,
        space_key: &str,
        dir: &std::path::Path,
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
    ) -> Result<i64> {
        let capabilities = self.projection_capabilities(subject).await?;
        let budget = snapshot.get(crate::EPISODE_SYNOPSIS)?;
        let epa_policy = snapshot.get(crate::EPA_POLICY)?;
        let provider = self
            .embedding()
            .ok_or_else(|| Error::Unavailable("text embedding provider is unavailable".into()))?;
        let space = provider.space();
        if space_key != space.space_hash {
            return Err(Error::Unavailable(
                "embedding space has no configured provider".into(),
            ));
        }
        let input = self
            .text_input_in_view(
                subject,
                "dense",
                space_key,
                capabilities.memory,
                budget,
                view,
            )
            .await?;
        let source_regions: std::collections::HashMap<_, _> = input
            .sources
            .iter()
            .map(|source| (source.reference.clone(), source.source_region))
            .collect();
        let documents = self.documents(subject, input.sources, budget, view).await?;
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
        for document in documents {
            let Some(output) = self
                .embed_document(
                    subject,
                    &document,
                    concepts.map(AsRef::as_ref),
                    provider.as_ref(),
                )
                .await?
            else {
                continue;
            };
            let source_region = source_regions
                .get(&document.reference)
                .copied()
                .flatten()
                .map(CognitiveRef::SourceRegion);
            let record = VectorRecord {
                serving_doc_id: document.serving_doc_id,
                representation_kind: if matches!(document.reference, CognitiveRef::Tag(_)) {
                    "tag"
                } else {
                    "text"
                }
                .into(),
                reference: document.reference,
                embedding_space: space.clone(),
                producer_signature: output.producer.signature_hash,
                source_region,
            };
            vectors.push((record, output.vector));
        }
        let directory = dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut generation = DenseGeneration::new(space.clone(), vectors.len())?;
            generation.generation_id = id;
            let mut tag_vectors = Vec::new();
            let mut records = Vec::new();
            for (record, vector) in vectors {
                if matches!(record.reference, CognitiveRef::Tag(_)) {
                    tag_vectors.push((
                        record.serving_doc_id,
                        vector.iter().map(|v| f64::from(*v)).collect(),
                    ));
                }
                generation.insert(record.clone(), &vector)?;
                records.push(record);
            }
            generation.save(&directory.join("vectors.usearch"))?;
            let basis =
                build_epa_basis(&tag_vectors, &space.space_hash, &epa_policy).map(|basis| {
                    EpaBasisGeneration {
                        generation_id: id,
                        basis,
                    }
                });
            write_json(
                &directory.join("records.json"),
                &DenseManifest {
                    space,
                    records,
                    basis,
                },
            )
        })
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))??;
        Ok(input.watermark)
    }
}

pub(crate) fn implementation(family: &str) -> &'static str {
    super::family::AssetFamily::parse(family)
        .map_or("unknown", super::family::AssetFamily::implementation)
}

fn topology_asset_profile(
    snapshot: &nous_configuration::ConfigSnapshot,
) -> Result<nous_runtime::CognitiveProfile> {
    let selected = snapshot.get(nous_runtime::COGNITIVE_PROFILE)?;
    Ok(match selected {
        nous_runtime::CognitiveProfile::VcpDtsc | nous_runtime::CognitiveProfile::VcpRiverMemo => {
            selected
        }
        _ => nous_runtime::CognitiveProfile::NousNodePotential,
    })
}
