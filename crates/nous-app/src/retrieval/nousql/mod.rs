mod bind;
mod directives;
mod execute;
pub(super) mod ranking;
use super::{config::RetrievalPolicy, *};
use crate::App;
use nous_domain::nousql::*;
use nous_foundation::clock::domains::{TIME_DOMAINS, TimeDomains};
use nous_storage::ReadView;
use std::{future::Future, pin::Pin};

struct Binder {
    read: ReadView,
    source_read: nous_storage::SourceReadView,
    namespace: NamespaceId,
    subject: SubjectId,
    clock: TimePoint,
    recorded: TimePoint,
    policy: RetrievalPolicy,
    time_domains: TimeDomains,
    leaves: Vec<PlannedQuery>,
}
type BindFuture<'a> = Pin<Box<dyn Future<Output = Result<PlannedExpression>> + Send + 'a>>;

impl App {
    pub async fn plan_query(&self, ctx: &CallContext, request: QueryRequest) -> Result<QueryPlan> {
        self.build_query_plan(ctx, request)
            .await
            .map(|(plan, _)| plan)
    }
    pub(super) async fn build_query_plan(
        &self,
        ctx: &CallContext,
        request: QueryRequest,
    ) -> Result<(QueryPlan, RetrievalPolicy)> {
        request.validate()?;
        let context = if let Some(scope) = &request.source_read {
            ctx.with_source_read(scope.clone())?
        } else {
            ctx.clone()
        };
        let namespace = self.database.subject_namespace(&request.subject_id).await?;
        context.require_subject(&namespace, &request.subject_id)?;
        let policy = self
            .retrieval_policy(&namespace, &request.subject_id)
            .await?;
        policy.admit(&request)?;
        let now = nous_foundation::clock::Clock::operation_snapshot();
        let clock = request.clock.clone().unwrap_or_else(|| now.point());
        let root = parse(&request.input.text)?;
        let mut read = self.database.read_view().await?;
        read.subject(&namespace, &request.subject_id).await?;
        let configuration = self
            .configuration
            .snapshot_in(
                &mut read,
                &ConfigScope::Subject {
                    namespace_id: namespace.clone(),
                    subject_id: request.subject_id.clone(),
                },
                false,
            )
            .await?;
        let time_domains = configuration.get(TIME_DOMAINS)?;
        let source_read = read
            .source_read_view(&context, &namespace, &request.subject_id)
            .await?;
        let mut cut = request.as_of_commit.unwrap_or(read.watermark);
        if let Some(asof) = root
            .directives
            .iter()
            .find(|directive| directive.name == "asof")
        {
            if request.as_of_commit.is_some() {
                return Err(Error::invalid(
                    "provide one Authority cut, not both as_of_commit and $asof",
                ));
            }
            let at = if asof.positional.len() == 1 && asof.named.is_empty() {
                if let Some(timestamp) = asof.positional[0].as_str() {
                    OperationTime::parse(timestamp)?
                } else {
                    OperationTime::new(
                        chrono::DateTime::from_timestamp_millis(time_coordinate(
                            &asof.positional[0],
                        )?)
                        .ok_or_else(|| Error::invalid("asof timestamp is out of range"))?,
                    )
                }
            } else if asof.positional.is_empty() && asof.named.len() == 1 {
                let duration = asof
                    .named
                    .get("ago")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::invalid("use $asof(timestamp) or $asof(ago=duration)"))?;
                OperationTime::new(
                    now.utc()
                        .checked_sub_signed(chrono::Duration::milliseconds(duration_ms(duration)?))
                        .ok_or_else(|| Error::invalid("asof timestamp overflow"))?,
                )
            } else {
                return Err(Error::invalid(
                    "use $asof(timestamp) or $asof(ago=duration)",
                ));
            };
            cut = read.commit_at_recorded(&at).await?;
        }
        if cut < 0 || cut > read.watermark {
            return Err(Error::invalid(
                "query Authority cut is outside committed history",
            ));
        }
        let mut leaf = PlannedQuery {
            effort: "normal".into(),
            direct_only: !root.cues.is_empty()
                && root.intent.is_empty()
                && !root
                    .cues
                    .iter()
                    .any(|cue| matches!(cue, QueryCue::Concept { .. }))
                && request.input.media.is_empty(),
            stats: None,
            request: request.clone(),
            canonical: root.canonical(),
            selectors: Vec::new(),
            preferences: Vec::new(),
            explore: false,
            materialize: false,
            diagnostics: "summary".into(),
            candidate_budget: policy.candidates(request.limit),
            query_sources: request
                .input
                .media
                .iter()
                .map(|reference| SourceReadRef::Media {
                    reference: reference.clone(),
                    observation_id: None,
                })
                .collect(),
            current_view: request.as_of_commit.is_none()
                && !request.history
                && !root
                    .directives
                    .iter()
                    .any(|directive| directive.name == "asof"),
            empty: false,
        };
        leaf.request.clock = Some(clock.clone());
        leaf.request.input.text.clear();
        leaf.request.expression = None;
        leaf.request.as_of_commit = Some(cut);
        directives::apply(
            &mut leaf,
            &root,
            true,
            &clock,
            &now.point(),
            &policy,
            &time_domains,
        )?;
        let mut binder = Binder {
            read,
            source_read,
            namespace,
            subject: request.subject_id.clone(),
            clock: clock.clone(),
            recorded: now.point(),
            policy,
            time_domains,
            leaves: Vec::new(),
        };
        binder.bind_selectors(&mut leaf, &root.cues).await?;
        binder
            .read
            .require_source_requirements(
                &context,
                &binder.namespace,
                &binder.subject,
                &leaf.query_sources,
            )
            .await?;
        let inline = binder.composition(&root)?;
        if inline.is_some() && request.expression.is_some() {
            return Err(Error::invalid("provide one query expression"));
        }
        let total_budget = leaf.candidate_budget;
        let root_sort = leaf.request.sort.clone();
        let expression = if let Some(expression) = inline.or(request.expression.clone()) {
            binder.node(expression, leaf, 0).await?
        } else {
            binder.push(leaf)
        };
        let ordinary = binder
            .leaves
            .iter()
            .filter(|leaf| leaf.request.reference.is_none())
            .count();
        let mut ordinal = 0;
        for leaf in &mut binder.leaves {
            if leaf.request.reference.is_none() {
                leaf.candidate_budget =
                    total_budget / ordinary + usize::from(ordinal < total_budget % ordinary);
                ordinal += 1;
            }
        }
        let policy = binder.policy.clone();
        let domains = binder
            .leaves
            .iter()
            .flat_map(|leaf| {
                leaf.request
                    .times
                    .iter()
                    .chain(leaf.request.time.iter())
                    .map(|time| time.range.domain.clone())
                    .chain(
                        leaf.request
                            .sort
                            .iter()
                            .filter_map(|sort| sort.domain.clone()),
                    )
            })
            .chain([clock.domain.clone(), "unix_ms".into()])
            .chain(root_sort.iter().filter_map(|sort| sort.domain.clone()))
            .collect::<BTreeSet<_>>();
        let time_units = domains
            .into_iter()
            .filter_map(|domain| {
                binder.time_domains.0.get(&domain).map(|scale| {
                    (
                        domain,
                        serde_json::to_value(scale).expect("time scale serializable"),
                    )
                })
            })
            .collect();
        binder.read.finish().await?;
        Ok((
            QueryPlan {
                rerank: request.rerank.unwrap_or(policy.rerank.mode),
                sort: root_sort,
                time_units,
                source: request.input.text,
                canonical: root.canonical(),
                authority_commit_seq: cut,
                clock,
                recorded_at: now.recorded(),
                expression,
                leaves: binder.leaves,
            },
            policy,
        ))
    }
}
