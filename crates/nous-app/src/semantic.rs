//! Authority-scoped semantic query planning and final materialization.
use super::*;
use crate::embeddings::{memory_input, provenance, text_input};
use nous_retrieval::{
    serving::{ServingGeneration, ServingMemory, ServingProfile, ServingSnapshot, ServingTag},
    vector::VectorGenerationIdentity,
};
use nous_storage::RetrievalEntry;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn eligible(entry: &RetrievalEntry, request: &QueryRequest) -> bool {
    !entry.memory.deleted
        && (request.include_inactive
            || entry.memory.lifecycle.activation == MemoryActivation::Active)
        && crate::cognition::matches_time(&entry.memory, request.time.as_ref())
        && request
            .source
            .as_ref()
            .is_none_or(|source| entry.material.source.as_ref() == Some(source))
}

impl App {
    pub(crate) async fn semantic_query(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        request: QueryRequest,
    ) -> Result<QueryResult> {
        let config = self.model.embedding_config().ok_or_else(|| {
            Error::new(
                ErrorCode::CapabilityUnavailable,
                "text embedding resource is not configured",
            )
        })?;
        let space = config.space();
        let sync = self
            .sync_embeddings_filtered(
                ctx,
                &ulid::Ulid::generate().to_string(),
                &request.subject_id,
                5000,
                Some(&request),
            )
            .await?;
        let jobs: Vec<String> = serde_json::from_value(sync.result["jobs"].clone())?;
        self.wait_embeddings(&jobs).await?;
        let query = self
            .query_embedding(ctx, namespace, &request.subject_id, &request.text)
            .await?;
        let snapshot = self
            .database
            .retrieval_snapshot(namespace, &request.subject_id, &space.id, 5000)
            .await?;
        if !snapshot.complete {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "semantic snapshot exceeds 5000 memories",
            ));
        }
        let entries: Vec<_> = snapshot
            .entries
            .into_iter()
            .filter(|entry| eligible(entry, &request))
            .collect();
        let eligible_memories = entries.len();
        let tags: BTreeMap<_, _> = snapshot
            .tag_embeddings
            .into_iter()
            .map(|record| (record.source_key.clone(), record))
            .collect();
        let mut missing_tags = BTreeSet::new();
        let mut memories = Vec::new();
        let mut records = BTreeMap::new();
        let mut watermark = 0;
        for entry in &entries {
            let Some(record) = &entry.embedding else {
                continue;
            };
            if record.source_hash != memory_input(&entry.memory).source_hash {
                continue;
            }
            let root = format!("material:{}", entry.material.id);
            let mut serving_tags = Vec::new();
            let mut seen = BTreeSet::new();
            for label in &entry.details.tags {
                let input = text_input(EmbeddingKind::Tag, label);
                if input.text.is_empty() || !seen.insert(input.source_key.clone()) {
                    continue;
                }
                if let Some(tag) = tags.get(&input.source_key) {
                    watermark = watermark.max(tag.commit_seq);
                    serving_tags.push(ServingTag {
                        key: input.source_key,
                        label: input.text,
                        vector: tag.vector.clone(),
                        source_ref: root.clone(),
                    });
                } else {
                    missing_tags.insert(input.source_key);
                }
            }
            watermark = watermark
                .max(entry.memory.commit_seq)
                .max(record.commit_seq);
            memories.push(ServingMemory {
                id: entry.memory.id.to_string(),
                revision: entry.memory.revision,
                commit_seq: entry.memory.commit_seq,
                vector: record.vector.clone(),
                source_ref: root,
                tags: serving_tags,
            });
            records.insert(entry.memory.id.clone(), record.clone());
        }
        let embedded_memories = memories.len();
        let identity = VectorGenerationIdentity {
            space: space.id.clone(),
            producer: space.producer_signature.clone(),
            generation: "nous.authority.snapshot.v1".into(),
            signature: format!("namespace:{namespace}/subject:{}", request.subject_id),
            watermark,
            dimension: space.dimension,
        };
        let input = ServingSnapshot::new(identity, memories);
        let cache_key = crate::embeddings::text_hash(&serde_json::to_string(&input)?);
        let parent = self
            .paths
            .resolve("indexes/semantic-v1")
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
                .map_err(semantic_error)?;
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
        let vector = query.vector.clone();
        let serving = tokio::task::spawn_blocking(move || {
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
        .map_err(semantic_error)?;
        let (serving, restored) = serving;
        let frozen: BTreeMap<_, _> = entries
            .iter()
            .map(|entry| (entry.memory.id.clone(), entry))
            .collect();
        let mut hits = Vec::new();
        for candidate in serving.ranked {
            let id: MemoryId = candidate.memory_id.parse()?;
            let entry = frozen
                .get(&id)
                .ok_or_else(|| Error::internal("serving candidate exceeds its query snapshot"))?;
            if entry.memory.revision != candidate.revision {
                return Err(Error::internal(
                    "serving candidate revision differs from its snapshot",
                ));
            }
            hits.push(QueryHit {
                reference: format!("memory:{}@{}", id, entry.memory.revision),
                material_id: entry.memory.material_id.clone(),
                observation_id: entry.memory.observation_id.clone(),
                source: entry.material.source.clone(),
                memory: entry.memory.clone(),
                score: candidate.score as f32,
            });
        }
        let mut complete = embedded_memories == eligible_memories && missing_tags.is_empty();
        let authority = snapshot.watermark;
        if mode == QueryMode::Hybrid {
            let mut lexical_request = request.clone();
            lexical_request.mode = QueryMode::Lexical;
            lexical_request.limit = limit;
            let lexical = self.lexical_query(namespace, lexical_request).await?;
            complete &= lexical.coverage.complete;
            let lexical_hits = lexical
                .hits
                .into_iter()
                .filter(|hit| {
                    frozen
                        .get(&hit.memory.id)
                        .is_some_and(|entry| entry.memory.revision == hit.memory.revision)
                })
                .collect();
            hits = fuse(hits, lexical_hits, request.limit);
        } else {
            hits.truncate(request.limit);
        }
        // Every lane shares the frozen identities; final authority policy is
        // applied once after fusion, immediately before publishing source bodies.
        let mut current_hits = Vec::new();
        let mut selected_provenance = Vec::new();
        for mut hit in hits {
            let current = match self.database.memory(namespace, &hit.memory.id).await {
                Ok(memory) => memory,
                Err(error) if error.code == ErrorCode::NotFound => {
                    complete = false;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if current.subject_id != request.subject_id
                || current.revision != hit.memory.revision
                || current.deleted
                || (!request.include_inactive
                    && current.lifecycle.activation != MemoryActivation::Active)
                || !crate::cognition::matches_time(&current, request.time.as_ref())
            {
                complete = false;
                continue;
            }
            let material = self
                .database
                .material(namespace, &current.material_id)
                .await?;
            if request
                .source
                .as_ref()
                .is_some_and(|source| material.source.as_ref() != Some(source))
            {
                complete = false;
                continue;
            }
            if let Some(record) = records.get(&current.id) {
                selected_provenance.push(provenance(record));
            }
            hit.memory = current;
            hit.source = material.source;
            current_hits.push(hit);
        }
        let hits = current_hits;
        Ok(QueryResult {
            hits,
            coverage: QueryCoverage {
                indexed_through: watermark,
                authority_commit_seq: authority,
                complete,
                generation: serving.generation.clone(),
            },
            semantic: Some(SemanticQueryInfo {
                mode,
                space_id: space.id,
                query_embedding_call_id: query.model_call_id,
                generation: serving.generation,
                eligible_memories,
                embedded_memories,
                missing_tags: missing_tags.len(),
                provenance: selected_provenance,
                diagnostics: json!({"restored":restored,"serving":serving.diagnostics}),
            }),
        })
    }
}

fn semantic_error(error: anyhow::Error) -> Error {
    Error::new(
        ErrorCode::DependencyUnavailable,
        format!("semantic retrieval: {error:#}"),
    )
}

// Adapted from PR #56 packages/nous/src/retrieval/index.ts reciprocalRankFusion,
// 584517a4abddea6d1140b64ab20914e793093a70. AGPL-3.0-only license retained in licenses/.
fn fuse(semantic: Vec<QueryHit>, lexical: Vec<QueryHit>, limit: usize) -> Vec<QueryHit> {
    let mut scores: BTreeMap<MemoryId, f32> = BTreeMap::new();
    let mut objects = BTreeMap::new();
    for ranking in [semantic, lexical] {
        let mut seen = BTreeSet::new();
        let mut rank = 0;
        for hit in ranking {
            if !seen.insert(hit.memory.id.clone()) {
                continue;
            }
            *scores.entry(hit.memory.id.clone()).or_default() += 1.0 / (60.0 + rank as f32 + 1.0);
            objects.insert(hit.memory.id.clone(), hit);
            rank += 1;
        }
    }
    let mut hits: Vec<_> = objects
        .into_iter()
        .map(|(id, mut hit)| {
            hit.score = scores[&id];
            hit
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.memory.id.as_str().cmp(b.memory.id.as_str()))
    });
    hits.truncate(limit);
    hits
}
