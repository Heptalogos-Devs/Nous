use super::view::{Entry, QueryView};
use super::*;
use nous_retrieval::IndexDocument;

impl App {
    pub async fn query(&self, ctx: &CallContext, request: QueryRequest) -> Result<QueryResult> {
        request.validate()?;
        let context = if let Some(scope) = &request.source_read { ctx.with_source_read(scope.clone())? } else { ctx.clone() };
        let ctx = &context;
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&namespace, &request.subject_id)?;
        let policy = self
            .retrieval_policy(&namespace, &request.subject_id)
            .await?;
        policy.admit(&request)?;
        let model = if request.mode == QueryMode::Lexical || request.reference.is_some() {
            None
        } else {
            Some(
                self.embedding_configuration(&namespace, &request.subject_id)
                    .await?,
            )
        };
        let space = model
            .as_ref()
            .map(|model| model.space().map(|space| space.id))
            .transpose()?
            .unwrap_or_default();
        let mut view = self
            .query_view(ctx, &namespace, &request, &space, &policy)
            .await?;
        if let Some(model) = &model {
            view.admit_routes(model)?;
        }
        if request
            .min_commit_seq
            .is_some_and(|required| required > view.watermark)
        {
            return Err(Error::invalid(
                "query minimum commit is outside its committed authority view",
            ));
        }
        let mut semantic = None;
        let (ranks, generation, mut complete) = if request.reference.is_some() {
            let ranks = view
                .selected(&request)
                .map(|entry| (entry.reference().to_string(), 1.0))
                .collect();
            (ranks, format!("authority:{}", view.watermark), true)
        } else if request.mode == QueryMode::Lexical || view.selected(&request).next().is_none() {
            let (ranks, generation) = self
                .lexical_ranks(&namespace, &request, &view, request.limit)
                .await?;
            (ranks, generation, true)
        } else {
            let model = model.as_ref().expect("semantic query resolved its model");
            self.ensure_query_vectors(ctx, &namespace, &request, &mut view, model)
                .await?;
            let (mut ranks, info, is_complete) = self
                .semantic_ranks(ctx, &namespace, &request, &view, model, &policy)
                .await?;
            if request.mode == QueryMode::Hybrid {
                let (lexical, _) = self
                    .lexical_ranks(
                        &namespace,
                        &request,
                        &view,
                        policy.candidates(request.limit),
                    )
                    .await?;
                ranks = fuse(ranks, lexical, request.limit, policy.rrf_constant);
            }
            let generation = info.generation.clone();
            semantic = Some(info);
            (ranks, generation, is_complete)
        };
        // Recreate the same authority view immediately before returning bodies.
        // Current queries require current exact heads; historical/exact reads retain their selected revisions.
        let current = self
            .query_view(ctx, &namespace, &request, "", &policy)
            .await?;
        let available = current
            .selected(&request)
            .map(|entry| (entry.reference().to_string(), entry))
            .collect::<BTreeMap<_, _>>();
        let frozen = view
            .selected(&request)
            .map(|entry| (entry.reference().to_string(), entry))
            .collect::<BTreeMap<_, _>>();
        let mut hits = Vec::new();
        let mut evidence = Vec::new();
        for (key, score) in ranks {
            let Some(entry) = available.get(&key).filter(|_| frozen.contains_key(&key)) else {
                complete = false;
                continue;
            };
            if let Some(record) = frozen
                .get(&key)
                .and_then(|entry| entry.accepted_embedding())
            {
                evidence.push(super::embedding::provenance(record));
            }
            if !entry.sources_complete {
                complete = false;
            }
            hits.push(QueryHit {
                reference: entry.reference(),
                object: entry.object.clone(),
                score,
                source_roots: entry.roots.clone(),
            });
            if hits.len() == request.limit {
                break;
            }
        }
        if let Some(info) = &mut semantic {
            info.provenance = evidence;
        }
        let selected = view
            .selected(&request)
            .map(Entry::reference)
            .collect::<BTreeSet<_>>();
        view.issues.retain(|issue| selected.contains(&issue.target));
        complete &= view.issues.is_empty();
        Ok(QueryResult {
            hits,
            semantic,
            coverage: QueryCoverage {
                indexed_through: view.watermark,
                authority_commit_seq: current.watermark,
                complete,
                generation,
                projection_issues: view.issues,
            },
        })
    }

    async fn lexical_ranks(
        &self,
        namespace: &NamespaceId,
        request: &QueryRequest,
        view: &QueryView,
        limit: usize,
    ) -> Result<(Vec<(String, f32)>, String)> {
        if request.input.text.trim().is_empty() {
            let mut entries = view.selected(request).collect::<Vec<_>>();
            entries.sort_by(|a, b| {
                b.object
                    .commit_seq()
                    .cmp(&a.object.commit_seq())
                    .then_with(|| a.reference().cmp(&b.reference()))
            });
            let ranks = entries
                .into_iter()
                .take(limit)
                .map(|entry| (entry.reference().to_string(), 1.0))
                .collect();
            return Ok((ranks, format!("authority:{}", view.watermark)));
        }
        let allowed = view
            .selected(request)
            .map(|entry| entry.reference().to_string())
            .collect::<BTreeSet<_>>();
        let documents = view
            .entries
            .iter()
            .filter_map(|entry| {
                entry.lexical_text.as_ref().map(|text| IndexDocument {
                    id: entry.reference().to_string(),
                    namespace_id: namespace.to_string(),
                    subject_id: request.subject_id.to_string(),
                    commit_seq: entry.object.commit_seq(),
                    text: text.clone(),
                    deleted: false,
                })
            })
            .collect::<Vec<_>>();
        let index = self.retrieval.clone();
        let namespace = namespace.to_string();
        let subject = request.subject_id.to_string();
        let text = request.input.text.clone();
        let watermark = view.watermark;
        let guard = self.index_gate.clone().lock_owned().await;
        nous_foundation::execution::blocking(
            &tokio_util::sync::CancellationToken::new(),
            move |_| {
                let _guard = guard;
                let coverage = index
                    .sync_scope(&namespace, &subject, &documents, watermark)
                    .map_err(engine_error)?;
                let page = index
                    .search_allowed(&text, &namespace, &subject, limit, &allowed)
                    .map_err(engine_error)?;
                Ok((
                    page.hits
                        .into_iter()
                        .map(|hit| (hit.id, hit.score))
                        .collect(),
                    coverage.generation,
                ))
            },
        )
        .await
    }
}

fn fuse(
    semantic: Vec<(String, f32)>,
    lexical: Vec<(String, f32)>,
    limit: usize,
    constant: f32,
) -> Vec<(String, f32)> {
    let mut scores = BTreeMap::<String, f32>::new();
    for lane in [semantic, lexical] {
        for (rank, (id, _)) in lane.into_iter().enumerate() {
            *scores.entry(id).or_default() += 1.0 / (constant + rank as f32 + 1.0);
        }
    }
    let mut ranks = scores.into_iter().collect::<Vec<_>>();
    ranks.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranks.truncate(limit);
    ranks
}
