use super::*;

impl App {
    pub async fn query(&self, ctx: &CallContext, request: QueryRequest) -> Result<QueryResult> {
        let context = if let Some(scope) = &request.source_read {
            ctx.with_source_read(scope.clone())?
        } else {
            ctx.clone()
        };
        let ctx = &context;
        let (mut plan, policy) = self.build_query_plan(ctx, request).await?;
        let mut results = Vec::new();
        for leaf in &mut plan.leaves {
            let (result, eligible) = if leaf.empty {
                (empty(plan.authority_commit_seq), 0)
            } else {
                self.execute_query_atom(ctx, leaf, &policy, plan.sort.as_ref())
                    .await?
            };
            leaf.stats = Some(QueryLeafStats {
                eligible_objects: eligible,
                offered_objects: result.hits.len(),
                truncated: result.hits.len() < eligible
                    && result.hits.len() >= leaf.candidate_budget,
                semantic: result.semantic.clone(),
            });
            results.push(result);
        }
        let limit = plan.leaves.first().map_or(0, |leaf| leaf.request.limit);
        let scores = combine(&plan.expression, &results, policy.rrf_constant);
        let mut bodies = BTreeMap::<String, QueryHit>::new();
        for hit in results.iter().flat_map(|result| &result.hits) {
            let entry = bodies
                .entry(hit.reference.to_string())
                .or_insert_with(|| hit.clone());
            let order = plan.sort.as_ref().map(|sort| {
                if sort
                    .compare(sort_point(hit, sort), sort_point(entry, sort))
                    .is_lt()
                {
                    hit.order.clone()
                } else {
                    entry.order.clone()
                }
            });
            if hit.score > entry.score {
                *entry = hit.clone();
            }
            if let Some(order) = order {
                entry.order = order;
            }
        }
        let mut hits = Vec::new();
        for (key, score) in scores {
            if let Some(mut hit) = bodies.remove(&key) {
                hit.score = score;
                hits.push(hit);
            }
        }
        let rerank = self.rerank_hits(ctx, &plan, &policy, &mut hits).await?;
        let (rechecked, authority_watermark) =
            if rerank.as_ref().is_some_and(|info| info.job_id.is_some()) {
                self.recheck_reranked_hits(ctx, &plan, &policy, &mut hits)
                    .await?
            } else {
                (true, plan.authority_commit_seq)
            };
        if let Some(sort) = &plan.sort {
            hits.sort_by(|left, right| {
                sort.compare(sort_point(left, sort), sort_point(right, sort))
                    .then_with(|| right.score.total_cmp(&left.score))
                    .then_with(|| left.reference.cmp(&right.reference))
            });
        }
        hits.truncate(limit);
        if plan.leaves.iter().any(|leaf| leaf.materialize) {
            let subject = plan.leaves[0].request.subject_id.clone();
            for hit in &mut hits {
                let value = match &hit.reference {
                    RetrievalTarget::Cognition(reference)
                        if reference.kind == CognitiveKind::Memory =>
                    {
                        serde_json::to_value(
                            self.read_cognitive_source(
                                ctx,
                                hit.object.namespace_id(),
                                &subject,
                                reference,
                            )
                            .await?,
                        )?
                    }
                    RetrievalTarget::Cognition(reference) => {
                        self.structured_source(
                            ctx,
                            &CognitiveId(reference.id.clone()),
                            Some(reference.revision),
                        )
                        .await?
                    }
                    RetrievalTarget::Media(reference) => serde_json::to_value(
                        self.media_source_lineage(ctx, &subject, reference).await?,
                    )?,
                };
                hit.materialized = Some(value);
            }
        }
        let mut complete = rechecked && results.iter().all(|result| result.coverage.complete);
        if (has_intersection(&plan.expression) || plan.sort.is_some())
            && plan
                .leaves
                .iter()
                .any(|leaf| leaf.stats.as_ref().is_some_and(|stats| stats.truncated))
        {
            complete = false;
        }
        let coverage = QueryCoverage {
            indexed_through: results
                .iter()
                .map(|result| result.coverage.indexed_through)
                .min()
                .unwrap_or(plan.authority_commit_seq),
            authority_commit_seq: results
                .iter()
                .map(|result| result.coverage.authority_commit_seq)
                .max()
                .unwrap_or(plan.authority_commit_seq)
                .max(authority_watermark),
            complete,
            generation: results
                .iter()
                .map(|result| result.coverage.generation.as_str())
                .collect::<Vec<_>>()
                .join("+"),
            projection_issues: results
                .iter()
                .flat_map(|result| result.coverage.projection_issues.clone())
                .collect(),
        };
        let semantic = if results.len() == 1 {
            results.pop().and_then(|result| result.semantic)
        } else {
            None
        };
        let diagnostics = plan
            .leaves
            .first()
            .is_some_and(|leaf| leaf.diagnostics != "none");
        Ok(QueryResult {
            rerank,
            hits,
            coverage,
            semantic,
            plan: diagnostics.then_some(plan),
        })
    }
}
fn sort_point<'a>(hit: &'a QueryHit, sort: &QuerySort) -> Option<&'a TimePoint> {
    hit.order
        .as_ref()
        .filter(|order| order.axis == sort.axis)
        .map(|order| &order.point)
}
fn empty(watermark: i64) -> QueryResult {
    QueryResult {
        rerank: None,
        hits: Vec::new(),
        semantic: None,
        plan: None,
        coverage: QueryCoverage {
            indexed_through: watermark,
            authority_commit_seq: watermark,
            complete: true,
            generation: format!("authority:{watermark}"),
            projection_issues: Vec::new(),
        },
    }
}
fn has_intersection(expression: &PlannedExpression) -> bool {
    match expression {
        PlannedExpression::Leaf { .. } => false,
        PlannedExpression::All { .. } => true,
        PlannedExpression::Any { children } => children.iter().any(has_intersection),
    }
}
fn combine(
    expression: &PlannedExpression,
    results: &[QueryResult],
    constant: f32,
) -> Vec<(String, f32)> {
    match expression {
        PlannedExpression::Leaf { index } => results[*index]
            .hits
            .iter()
            .map(|hit| (hit.reference.to_string(), hit.score))
            .collect(),
        PlannedExpression::Any { children } | PlannedExpression::All { children } => {
            let all = matches!(expression, PlannedExpression::All { .. });
            let lanes = children
                .iter()
                .map(|child| combine(child, results, constant))
                .collect::<Vec<_>>();
            let allowed = if all {
                let mut sets = lanes.iter().map(|lane| {
                    lane.iter()
                        .map(|(key, _)| key.clone())
                        .collect::<BTreeSet<_>>()
                });
                sets.next().map(|mut set| {
                    for next in sets {
                        set.retain(|key| next.contains(key));
                    }
                    set
                })
            } else {
                None
            };
            crate::retrieval::query::rrf(
                lanes.into_iter().map(|lane| (lane, 1.0)),
                usize::MAX,
                constant,
            )
            .into_iter()
            .filter(|(key, _)| allowed.as_ref().is_none_or(|allowed| allowed.contains(key)))
            .collect()
        }
    }
}
