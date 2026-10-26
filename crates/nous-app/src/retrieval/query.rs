use super::view::{Entry, QueryView};
use super::*;
use nous_retrieval::IndexDocument;

impl App {
    pub(super) async fn execute_query_atom(
        &self,
        ctx: &CallContext,
        leaf: &nous_domain::nousql::PlannedQuery,
        policy: &super::config::RetrievalPolicy,
        root_sort: Option<&QuerySort>,
    ) -> Result<(QueryResult, usize)> {
        let mut request = leaf.request.clone();
        request.limit = leaf.candidate_budget.max(1);
        request.validate()?;
        let context = if let Some(scope) = &request.source_read {
            ctx.with_source_read(scope.clone())?
        } else {
            ctx.clone()
        };
        let ctx = &context;
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        ctx.require_subject(&namespace, &request.subject_id)?;
        let model = if request.mode == QueryMode::Lexical
            || request.reference.is_some()
            || leaf.candidate_budget == 0
            || (leaf.direct_only && !leaf.explore)
        {
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
        let mut view_request = request.clone();
        if leaf.direct_only && !leaf.explore {
            view_request.input.text.clear();
        }
        let mut view = self
            .query_view(ctx, &namespace, &view_request, &space, policy)
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
        let eligible_count = view.selected(&request).count();
        let mut semantic = None;
        let (mut ranks, generation, mut complete) = if leaf.candidate_budget == 0 {
            (
                Vec::new(),
                format!("authority:{}", view.watermark),
                eligible_count == 0,
            )
        } else if request.reference.is_some() {
            let ranks = view
                .selected(&request)
                .map(|entry| (entry.reference().to_string(), 1.0))
                .collect();
            (ranks, format!("authority:{}", view.watermark), true)
        } else if leaf.direct_only && !leaf.explore {
            (
                super::nousql::ranking::direct(&view, &request, leaf),
                format!("authority:{}", view.watermark),
                true,
            )
        } else if request.sort.is_some()
            && request.input.text.is_empty()
            && request.mode == QueryMode::Lexical
        {
            (
                view.selected(&request)
                    .map(|entry| (entry.reference().to_string(), 1.0))
                    .collect(),
                format!("authority:{}", view.watermark),
                true,
            )
        } else if request.mode == QueryMode::Lexical || view.selected(&request).next().is_none() {
            let (ranks, generation) = self
                .lexical_ranks(&namespace, &request, &view, request.limit)
                .await?;
            (ranks, generation, true)
        } else {
            let model = model.as_ref().expect("semantic query resolved its model");
            self.ensure_query_vectors(ctx, &namespace, leaf, &mut view, model)
                .await?;
            let (mut ranks, info, is_complete) = self
                .semantic_ranks(ctx, &namespace, leaf, &view, model, policy)
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
        if !leaf.direct_only && !leaf.selectors.is_empty() && request.reference.is_none() {
            let direct = super::nousql::ranking::direct(&view, &request, leaf);
            if !direct.is_empty() {
                ranks = rrf(
                    [(ranks, 1.0), (direct, 1.0)],
                    usize::MAX,
                    policy.rrf_constant,
                );
            }
        }
        ranks = super::nousql::ranking::order(&view, &request, leaf, ranks, policy.rrf_constant);
        ranks.truncate(leaf.candidate_budget);
        let mut final_request = request.clone();
        if leaf.direct_only && !leaf.explore {
            final_request.input.text.clear();
        }
        if leaf.current_view {
            final_request.as_of_commit = None;
        }
        // Recreate the same authority view immediately before returning bodies.
        // Current queries require current exact heads; historical/exact reads retain their selected revisions.
        let current = self
            .query_view(ctx, &namespace, &final_request, "", policy)
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
                order: root_sort.or(request.sort.as_ref()).and_then(|sort| {
                    let mut ordering = request.clone();
                    ordering.sort = Some(sort.clone());
                    frozen
                        .get(&key)
                        .and_then(|entry| entry.sort_time(&sort.axis, &ordering))
                        .map(|point| QueryHitOrder {
                            axis: sort.axis.clone(),
                            point,
                        })
                }),
                reference: entry.reference(),
                object: entry.object.clone(),
                score,
                source_roots: entry.roots.clone(),
                materialized: None,
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
        Ok((
            QueryResult {
                plan: None,
                hits,
                semantic,
                coverage: QueryCoverage {
                    indexed_through: view.watermark,
                    authority_commit_seq: current.watermark,
                    complete,
                    generation,
                    projection_issues: view.issues,
                },
            },
            eligible_count,
        ))
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
    rrf([(semantic, 1.0), (lexical, 1.0)], limit, constant)
}
pub(super) fn rrf(
    lanes: impl IntoIterator<Item = (Vec<(String, f32)>, f32)>,
    limit: usize,
    constant: f32,
) -> Vec<(String, f32)> {
    let mut scores = BTreeMap::<String, f32>::new();
    for (lane, weight) in lanes {
        for (rank, (id, _)) in lane.into_iter().enumerate() {
            *scores.entry(id).or_default() += weight / (constant + rank as f32 + 1.0);
        }
    }
    let mut ranks = scores.into_iter().collect::<Vec<_>>();
    ranks.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranks.truncate(limit);
    ranks
}
