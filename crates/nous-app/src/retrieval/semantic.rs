//! One frozen candidate/source graph for Dense, VCP and Hybrid discovery.
use super::view::QueryView;
use super::*;
use crate::common::models::ResolvedModel;
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
        request: &QueryRequest,
        view: &mut QueryView,
        config: &ResolvedModel,
    ) -> Result<()> {
        let inputs = view.pending_inputs(request)?;
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
        self.wait_embeddings(ctx, &jobs).await?;
        *view = self
            .query_view(namespace, request, &config.space()?.id)
            .await?;
        Ok(())
    }

    pub(super) async fn semantic_ranks(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        request: &QueryRequest,
        view: &QueryView,
        config: &ResolvedModel,
    ) -> Result<(Vec<(String, f32)>, SemanticQueryInfo, bool)> {
        let space = config.space()?;
        let query = self
            .query_embedding(ctx, namespace, &request.subject_id, &request.text, config)
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
                        label: input.text,
                        vector: record.vector.clone(),
                        source_ref: source,
                    });
                } else {
                    missing_tags.insert(key);
                }
            }
            watermark = watermark
                .max(entry.object.commit_seq())
                .max(record.commit_seq);
            candidates.push(ServingCandidate {
                id: entry.reference().to_string(),
                revision: entry.reference().revision,
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
                producer: space.producer_signature.clone(),
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
        let generation = {
            let mut cache = self.semantic_cache.lock().await;
            if let Some(generation) = cache.get(&cache_key) {
                generation.clone()
            } else {
                let generation = tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&parent)?;
                    ServingGeneration::create_or_open(&parent, &input)
                })
                .await
                .map_err(|error| Error::internal(error.to_string()))?
                .map_err(engine_error)?;
                let generation = Arc::new(generation);
                if cache.len() >= 8 {
                    cache.clear();
                }
                cache.insert(cache_key, generation.clone());
                generation
            }
        };
        let mode = request.mode;
        let limit = if mode == QueryMode::Hybrid {
            (request.limit * 8).min(800)
        } else {
            request.limit
        };
        let vector = query.vector;
        let (result, restored) = tokio::task::spawn_blocking(move || {
            let profile = if mode == QueryMode::Dense {
                ServingProfile::Dense
            } else {
                ServingProfile::Vcp(Box::default())
            };
            let restored = generation.restored();
            generation
                .search(&vector, limit, profile)
                .map(|result| (result, restored))
        })
        .await
        .map_err(|error| Error::internal(error.to_string()))?
        .map_err(engine_error)?;
        let allowed = eligible
            .iter()
            .map(|entry| (entry.reference().to_string(), entry.reference().revision))
            .collect::<BTreeMap<_, _>>();
        let mut ranks = Vec::new();
        for hit in result.ranked {
            if allowed.get(&hit.id) != Some(&hit.revision) {
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
