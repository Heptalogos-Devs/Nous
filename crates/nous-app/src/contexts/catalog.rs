use super::*;
use std::collections::BTreeSet;

pub(super) struct CapturedContext {
    pub policy: ContextConsumerPolicy,
    pub config_digest: String,
    pub catalog: Vec<ContextSegment>,
    pub work_context: Option<WorkContext>,
    pub session_revision: i64,
    pub watermark: i64,
    pub degradation: Vec<String>,
}
impl App {
    pub(super) async fn capture_context(
        &self,
        ctx: &CallContext,
        ns: &NamespaceId,
        request: &ContextPrepareRequest,
    ) -> Result<CapturedContext> {
        let (policy, policy_digest) = self
            .context_policy(ns, &request.subject_id, &request.consumer_id)
            .await?;
        let config_digest = super::compile::hash(
            &json!({"policy":policy_digest,"source_read":ctx.source_read_identity()?}),
        )?;
        let session = self.get_context_session(ctx, &request.session_id).await?;
        if session.subject_id != request.subject_id
            || session.namespace_id != *ns
            || session.closed_at.is_some()
            || session.history_invalidated
        {
            return Err(history_error(&session));
        }
        for target in self.database.context_history_targets(&session.id).await? {
            if !policy.allows(&target) {
                return Err(history_error(&session));
            }
            let mut read = self.database.read_view().await?;
            if read
                .require_source_read(ctx, ns, &request.subject_id, &target)
                .await
                .is_err()
            {
                return Err(history_error(&session));
            }
            read.finish().await?;
        }
        let work_context = if let Some(id) = &session.active_work_context {
            let context = self.get_work_context(ctx, id).await?;
            if context.state != WorkContextState::Open {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "foreground WorkContext is not open",
                ));
            }
            Some(context)
        } else {
            None
        };
        let mut catalog = Vec::new();
        let mut seen = BTreeSet::new();
        let mut degradation = Vec::new();
        let mut recalled = Vec::new();
        if let Some(context) = &work_context {
            catalog.push(ContextSegment {
                key: "work".into(),
                text: format!(
                    "目标：{}\n进展：{}\n未解决：{}\n约束：{}\n恢复条件：{}\n预算摘要：{}",
                    context.content.purpose,
                    context.content.context_text,
                    context.content.unresolved_questions.join("；"),
                    context.content.constraints,
                    context.content.resume_conditions.join("；"),
                    context.content.budget_summary
                ),
                sources: Vec::new(),
                dependencies: context.content.anchors.clone(),
                required: true,
                authority: ContextAuthority::TaskDirective,
                provenance: json!({"work_context_id":context.id,"revision":context.revision}),
                epistemic: None,
            });
        }
        for target in request.current_sources.iter().chain(
            work_context
                .iter()
                .flat_map(|context| &context.content.anchors),
        ) {
            if !policy.allows(target) {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "required context source is excluded by consumer policy",
                ));
            }
            if seen.insert(target.clone()) {
                catalog.push(
                    self.context_segment(ctx, ns, &request.subject_id, target, true)
                        .await?,
                );
            }
        }
        for resident in session.residents.iter().take(policy.max_residents) {
            let target = match self
                .current_resident_target(ctx, &request.subject_id, &resident.target)
                .await
            {
                Ok(Some(target)) => target,
                Ok(None) => continue,
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::NotFound | ErrorCode::Conflict | ErrorCode::Forbidden
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            if policy.allows(&target) && seen.insert(target.clone()) {
                match self
                    .context_segment(ctx, ns, &request.subject_id, &target, false)
                    .await
                {
                    Ok(source) => catalog.push(source),
                    Err(error)
                        if matches!(
                            error.code,
                            ErrorCode::NotFound
                                | ErrorCode::Forbidden
                                | ErrorCode::Conflict
                                | ErrorCode::CapabilityUnavailable
                        ) =>
                    {
                        degradation.push(format!("resident unavailable: {}", resident.target))
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let mut query = request.query.clone().unwrap_or_else(|| QueryRequest {
            rerank: None,
            sort: None,
            expression: None,
            clock: None,
            history: false,
            filters: Default::default(),
            times: Vec::new(),
            source_read: None,
            mode: QueryMode::Lexical,
            subject_id: request.subject_id.clone(),
            input: QueryInput {
                text: request.usage.intent.clone(),
                media: Vec::new(),
            },
            reference: None,
            domains: Vec::new(),
            kinds: Vec::new(),
            source: None,
            time: None,
            limit: policy.max_items.min(request.max_items),
            min_commit_seq: None,
            as_of_commit: None,
            include_inactive: false,
            include_deleted: false,
        });
        query.limit = query.limit.min(policy.max_items).min(request.max_items);
        query.include_deleted = false;
        let permitted_domains = query.domains.is_empty()
            || query.domains.contains(&QueryDomain::Cognition)
            || policy.allow_media;
        if !policy.allow_media {
            query.domains = vec![QueryDomain::Cognition];
        }
        let mut permitted_kinds = true;
        if !policy.allowed_kinds.is_empty() {
            if query.kinds.is_empty() {
                query.kinds = policy.allowed_kinds.clone();
            } else {
                query
                    .kinds
                    .retain(|kind| policy.allowed_kinds.contains(kind));
                permitted_kinds = !query.kinds.is_empty();
            }
        }
        if permitted_domains && permitted_kinds {
            match self.query(ctx, query).await {
                Ok(found) => {
                    for hit in found.hits {
                        recalled.push(hit.reference.clone());
                        if policy.allows(&hit.reference) && seen.insert(hit.reference.clone()) {
                            match self
                                .context_segment(
                                    ctx,
                                    ns,
                                    &request.subject_id,
                                    &hit.reference,
                                    false,
                                )
                                .await
                            {
                                Ok(source) => catalog.push(source),
                                Err(error)
                                    if matches!(
                                        error.code,
                                        ErrorCode::NotFound
                                            | ErrorCode::Conflict
                                            | ErrorCode::CapabilityUnavailable
                                    ) =>
                                {
                                    degradation.push(format!(
                                        "retrieved source unavailable: {}",
                                        hit.reference
                                    ))
                                }
                                Err(error) => return Err(error),
                            }
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::CapabilityUnavailable
                            | ErrorCode::ResourceExhausted
                            | ErrorCode::DependencyUnavailable
                    ) =>
                {
                    degradation.push(format!("retrieval unavailable: {}", error.message))
                }
                Err(error) => return Err(error),
            }
        }
        let max_items = policy.max_items.min(request.max_items);
        catalog.sort_by_key(|segment| {
            (
                !segment.required,
                segment
                    .sources
                    .iter()
                    .filter_map(|target| recalled.iter().position(|reference| reference == target))
                    .min()
                    .unwrap_or(usize::MAX),
            )
        });
        let max_bytes = policy.max_text_bytes.min(request.max_text_bytes);
        let required = catalog
            .iter()
            .filter(|source| source.required)
            .collect::<Vec<_>>();
        if required.len() > max_items
            || required
                .iter()
                .map(|source| source.text.len())
                .sum::<usize>()
                > max_bytes
        {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "required context units exceed the contribution budget",
            ));
        }
        let mut selected = Vec::new();
        let mut bytes = 0;
        for segment in catalog
            .iter()
            .filter(|segment| segment.required)
            .chain(catalog.iter().filter(|segment| !segment.required))
        {
            if selected.len() < max_items && bytes + segment.text.len() <= max_bytes {
                let mut segment = segment.clone();
                segment.key = format!("u{}", selected.len());
                bytes += segment.text.len();
                selected.push(segment);
            } else {
                degradation
                    .push("optional semantic unit omitted whole by contribution budget".into());
            }
        }
        Ok(CapturedContext {
            policy,
            config_digest,
            catalog: selected,
            work_context,
            session_revision: session.revision,
            watermark: self.database.status().await?.commit_seq,
            degradation,
        })
    }
}
pub(super) fn history_error(session: &ContextSession) -> Error {
    Error::new(ErrorCode::Conflict,"native Session history requires a new generation after source withdrawal or narrower scope")
        .with_context("history_reset_required",true).with_context("session_id",session.id.to_string())
}
