//! One frozen candidate/source graph for Dense, VCP and Hybrid discovery.
use super::view::QueryView;
use super::*;
use nous_foundation::models::ResolvedModel;
use nous_retrieval::{
    serving::{ServingCandidate, ServingGeneration, ServingProfile, ServingSnapshot, ServingTag},
    vector::VectorGenerationIdentity,
};
use sha2::{Digest, Sha256};

impl App {
    pub(super) async fn ensure_query_vectors(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        leaf: &nous_domain::nousql::PlannedQuery,
        view: &mut QueryView,
        config: &ResolvedModel,
    ) -> Result<()> {
        let request = &leaf.request;
        let mut inputs = view.pending_inputs(request)?;
        let activation = super::nousql::ranking::activations(leaf);
        for entry in view.entries.iter().filter(|entry| {
            activation.contains(&entry.reference())
                && entry.can_embed()
                && entry.accepted_embedding().is_none()
        }) {
            if let Some(input) = &entry.input
                && !inputs.iter().any(|known| {
                    known.target == input.target && known.source_hash() == input.source_hash()
                })
            {
                inputs.push(input.clone());
            }
        }
        if inputs.is_empty() {
            return Ok(());
        }
        let receipt = self
            .queue_embedding_inputs(
                ctx,
                &ulid::Ulid::generate().to_string(),
                namespace,
                &request.subject_id,
                inputs,
                config,
            )
            .await?;
        let jobs: Vec<String> = serde_json::from_value(receipt.result["jobs"].clone())?;
        self.wait_embeddings(ctx, namespace, &request.subject_id, &jobs)
            .await?;
        view.refresh_embeddings(
            &self.database,
            namespace,
            &request.subject_id,
            &config.space()?.id,
        )
        .await?;
        view.admit_routes(config)?;
        Ok(())
    }

    pub(super) async fn semantic_ranks(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        leaf: &nous_domain::nousql::PlannedQuery,
        view: &QueryView,
        config: &ResolvedModel,
        policy: &super::config::RetrievalPolicy,
    ) -> Result<(Vec<(String, f32)>, SemanticQueryInfo, bool)> {
        let mut physical = leaf.request.clone();
        physical.limit = leaf.candidate_budget.max(1);
        let request = &physical;
        let space = config.space()?;
        let mut parts = Vec::new();
        if !request.input.text.trim().is_empty() {
            parts.push(EmbeddingPart::Text {
                text: request.input.text.trim().to_owned(),
                source: None,
            });
        }
        for reference in &request.input.media {
            parts.extend(
                self.embedding_media_parts(
                    ctx,
                    namespace,
                    &request.subject_id,
                    reference,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await?,
            );
        }
        let query = self
            .query_embedding(
                ctx,
                namespace,
                &request.subject_id,
                parts,
                &leaf.query_sources,
                config,
            )
            .await?;
        let eligible = view.selected(request).collect::<Vec<_>>();
        let mut candidates = Vec::new();
        let mut missing_tags = BTreeSet::new();
        let mut watermark = 0;
        for entry in &eligible {
            if !entry.sources_complete {
                continue;
            }
            let Some(record) = entry.accepted_embedding() else {
                continue;
            };
            if entry.roots.is_empty() {
                continue;
            }
            let mut tags = Vec::new();
            for (source, input) in entry.tags()? {
                let key = input.source_key()?;
                if let Some(record) = view.tags.get(&key) {
                    watermark = watermark.max(record.commit_seq);
                    tags.push(ServingTag {
                        key,
                        label: input
                            .text_content()
                            .expect("tag has one text part")
                            .to_owned(),
                        vector: record.vector.clone(),
                        source_ref: source,
                    });
                } else {
                    missing_tags.insert(key);
                }
            }
            for selector in leaf.selectors.iter().filter(|selector| {
                selector.selector == nous_domain::nousql::SelectorKind::Tag
                    && selector.targets.contains(&entry.reference())
            }) {
                for canonical in &selector.canonical {
                    let Some(tag) = view
                        .entries
                        .iter()
                        .find(|candidate| candidate.reference().to_string() == *canonical)
                        .and_then(|candidate| candidate.accepted_embedding())
                    else {
                        continue;
                    };
                    for root in &entry.roots {
                        tags.push(ServingTag {
                            key: canonical.clone(),
                            label: canonical.clone(),
                            vector: tag.vector.clone(),
                            source_ref: format!("material:{root}"),
                        });
                    }
                    watermark = watermark.max(tag.commit_seq);
                }
            }
            watermark = watermark
                .max(entry.object.commit_seq())
                .max(record.commit_seq);
            candidates.push(ServingCandidate {
                id: entry.reference().to_string(),
                commit_seq: entry.object.commit_seq(),
                vector: record.vector.clone(),
                source_refs: entry
                    .roots
                    .iter()
                    .map(|root| format!("material:{root}"))
                    .collect(),
                tags,
            });
        }
        let embedded_objects = candidates.len();
        let input = ServingSnapshot::new(
            VectorGenerationIdentity {
                space: space.id.clone(),
                generation: String::new(),
                signature: String::new(),
                watermark,
                dimension: space.dimension,
            },
            candidates,
        );
        let cache_key = format!("{:x}", Sha256::digest(serde_json::to_vec(&input)?));
        let parent = self
            .paths
            .resolve("indexes/semantic")
            .map_err(|error| Error::invalid(error.to_string()))?;
        let cache_capacity = policy.semantic_cache_generations;
        let generation = {
            let mut cache = self.semantic_cache.clone().lock_owned().await;
            if let Some(generation) = cache.get(&cache_key) {
                generation.clone()
            } else {
                nous_foundation::execution::blocking(
                    &tokio_util::sync::CancellationToken::new(),
                    move |_| {
                        std::fs::create_dir_all(&parent)?;
                        let generation = Arc::new(
                            ServingGeneration::create_or_open(&parent, &input)
                                .map_err(engine_error)?,
                        );
                        if cache.len() >= cache_capacity {
                            cache.clear();
                        }
                        cache.insert(cache_key, generation.clone());
                        Ok(generation)
                    },
                )
                .await?
            }
        };
        let mode = request.mode;
        let limit = if mode == QueryMode::Hybrid {
            policy.candidates(request.limit)
        } else {
            request.limit
        };
        let vector = query.vector;
        let core_tags = leaf
            .selectors
            .iter()
            .filter(|selector| selector.selector == nous_domain::nousql::SelectorKind::Tag)
            .flat_map(|selector| selector.canonical.clone())
            .collect::<Vec<_>>();
        let diagnostics = leaf.diagnostics.clone();
        let (result, restored) = nous_foundation::execution::blocking(
            &tokio_util::sync::CancellationToken::new(),
            move |_| {
                let profile = if mode == QueryMode::Dense {
                    ServingProfile::Dense
                } else {
                    let config = nous_retrieval::serving::VcpConfig {
                        candidate_limit: limit,
                        core_tags,
                        include_trace: diagnostics == "full",
                        ..Default::default()
                    };
                    ServingProfile::Vcp(Box::new(config))
                };
                let restored = generation.restored();
                generation
                    .search(&vector, limit, profile)
                    .map(|result| (result, restored))
                    .map_err(engine_error)
            },
        )
        .await?;
        let allowed = eligible
            .iter()
            .map(|entry| entry.reference().to_string())
            .collect::<BTreeSet<_>>();
        let mut ranks = Vec::new();
        for hit in result.ranked {
            if !allowed.contains(&hit.id) {
                return Err(Error::internal(
                    "semantic candidate escaped the frozen authority view",
                ));
            }
            ranks.push((hit.id, hit.score as f32));
        }
        let complete = embedded_objects == eligible.len() && missing_tags.is_empty();
        Ok((
            ranks,
            SemanticQueryInfo {
                mode,
                space_id: space.id,
                query_embedding_call_id: query.model_call_id,
                generation: result.generation,
                eligible_objects: eligible.len(),
                embedded_objects,
                missing_tags: missing_tags.len(),
                provenance: vec![],
                diagnostics: json!({"restored":restored,"serving":result.diagnostics,"source_mappings":result.source_mappings}),
            },
            complete,
        ))
    }
}
