use super::*;
use nous_storage::ReferenceViewOptions;
impl Binder {
    pub(super) fn composition(&self, syntax: &QuerySyntax) -> Result<Option<QueryExpression>> {
        let values = syntax
            .directives
            .iter()
            .filter(|directive| matches!(directive.name.as_str(), "any" | "all"))
            .collect::<Vec<_>>();
        if values.len() > 1 {
            return Err(Error::invalid("one scope has one expression composition"));
        }
        let Some(directive) = values.first() else {
            return Ok(None);
        };
        let args = directive.strings()?;
        if args.len() < 2 || args.len() > 32 {
            return Err(Error::invalid(
                "expression composition requires 2..32 subqueries",
            ));
        }
        let children = args
            .into_iter()
            .map(|source| QueryExpression::Atom {
                source: source.into(),
            })
            .collect();
        Ok(Some(if directive.name == "any" {
            QueryExpression::Any { children }
        } else {
            QueryExpression::All { children }
        }))
    }
    pub(super) fn node(
        &mut self,
        node: QueryExpression,
        parent: PlannedQuery,
        depth: usize,
    ) -> BindFuture<'_> {
        Box::pin(async move {
            if depth > 16 || self.leaves.len() >= 64 {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "query expression exceeds 16 levels or 64 leaves",
                ));
            }
            match node {
                QueryExpression::Atom { source } => {
                    let syntax = parse(&source)?;
                    let mut leaf = parent;
                    directives::apply(
                        &mut leaf,
                        &syntax,
                        false,
                        &self.clock,
                        &self.recorded,
                        &self.policy,
                        &self.time_domains,
                    )?;
                    leaf.canonical = format!("{} {}", leaf.canonical, syntax.canonical());
                    self.bind_selectors(&mut leaf, &syntax.cues).await?;
                    if let Some(composition) = self.composition(&syntax)? {
                        self.node(composition, leaf, depth + 1).await
                    } else {
                        Ok(self.push(leaf))
                    }
                }
                node @ QueryExpression::Any { .. } | node @ QueryExpression::All { .. } => {
                    let (all, children) = match node {
                        QueryExpression::Any { children } => (false, children),
                        QueryExpression::All { children } => (true, children),
                        _ => unreachable!("expression group"),
                    };
                    if children.len() < 2 || children.len() > 32 {
                        return Err(Error::invalid("expression needs 2..32 children"));
                    }
                    let mut planned = Vec::new();
                    for child in children {
                        planned.push(self.node(child, parent.clone(), depth + 1).await?);
                    }
                    Ok(if all {
                        PlannedExpression::All { children: planned }
                    } else {
                        PlannedExpression::Any { children: planned }
                    })
                }
            }
        })
    }
    pub(super) fn push(&mut self, mut leaf: PlannedQuery) -> PlannedExpression {
        let mut exact = leaf
            .selectors
            .iter()
            .filter(|selector| selector.selector == SelectorKind::Ref)
            .flat_map(|selector| selector.targets.clone())
            .collect::<BTreeSet<_>>();
        if let Some(reference) = &leaf.request.reference {
            exact.insert(reference.clone());
        }
        if exact.len() > 1 {
            let children = exact
                .into_iter()
                .map(|reference| {
                    let mut local = leaf.clone();
                    local.request.reference = Some(reference);
                    local.current_view = false;
                    local.candidate_budget = 1;
                    let index = self.leaves.len();
                    self.leaves.push(local);
                    PlannedExpression::Leaf { index }
                })
                .collect();
            return PlannedExpression::Any { children };
        }
        if let Some(reference) = exact.into_iter().next() {
            leaf.request.reference = Some(reference);
            leaf.current_view = false;
            leaf.candidate_budget = 1;
        }
        leaf.candidate_budget = leaf
            .candidate_budget
            .max(leaf.request.limit)
            .min(self.policy.max_candidates);
        let index = self.leaves.len();
        self.leaves.push(leaf);
        PlannedExpression::Leaf { index }
    }
    pub(super) async fn bind_selectors(
        &mut self,
        leaf: &mut PlannedQuery,
        cues: &[QueryCue],
    ) -> Result<()> {
        for cue in cues {
            let QueryCue::Selector { selector, locators } = cue else {
                continue;
            };
            let mut canonical = Vec::new();
            let mut terms = Vec::new();
            let targets;
            match selector {
                SelectorKind::Ref => {
                    let reference = self
                        .read
                        .resolve_reference_locator(
                            &self.namespace,
                            &self.subject,
                            &locators[0],
                            &nous_storage::ReferenceViewOptions {
                                as_of: leaf.request.as_of_commit,
                                source_read: Some(&self.source_read),
                            },
                        )
                        .await?;
                    self.validate_exact(&reference, leaf).await?;
                    canonical.push(reference.to_string());
                    targets = vec![reference];
                }
                SelectorKind::Resource => {
                    let target = self
                        .read
                        .resolve_reference_locator(
                            &self.namespace,
                            &self.subject,
                            &locators[0],
                            &nous_storage::ReferenceViewOptions {
                                as_of: leaf.request.as_of_commit,
                                source_read: Some(&self.source_read),
                            },
                        )
                        .await?;
                    if target.media().is_none() {
                        return Err(Error::invalid("@r requires a media resource reference"));
                    }
                    self.validate_exact(&target, leaf).await?;
                    canonical.push(target.to_string());
                    targets = vec![target];
                }
                SelectorKind::Object => {
                    let reference = locators[0].parse::<ExternalObjectRef>()?;
                    canonical.push(reference.to_string());
                    targets = self
                        .read
                        .query_external_object_targets(
                            &self.namespace,
                            &self.subject,
                            &reference,
                            leaf.request.as_of_commit,
                            leaf.request.history,
                        )
                        .await?
                        .into_iter()
                        .collect();
                }
                kind => {
                    let kind = match kind {
                        SelectorKind::Entity => CognitiveKind::Entity,
                        SelectorKind::Tag => CognitiveKind::Concept,
                        SelectorKind::Schema => CognitiveKind::Schema,
                        _ => unreachable!("identity selector"),
                    };
                    let mut identities = Vec::new();
                    for locator in locators {
                        let identity = self
                            .read
                            .resolve_query_identity(
                                &self.namespace,
                                &self.subject,
                                kind,
                                locator,
                                &ReferenceViewOptions {
                                    as_of: leaf.request.as_of_commit,
                                    source_read: Some(&self.source_read),
                                },
                            )
                            .await?;
                        if identities.contains(&identity.reference) {
                            return Err(Error::invalid("duplicate identity participant"));
                        }
                        canonical.push(identity.reference.to_string());
                        terms.push(identity.label);
                        identities.push(identity.reference);
                    }
                    targets = self
                        .read
                        .query_identity_targets(
                            &self.namespace,
                            &self.subject,
                            &identities,
                            leaf.request.as_of_commit,
                            leaf.request.history,
                        )
                        .await?
                        .into_iter()
                        .collect();
                }
            }
            if !terms.is_empty() {
                leaf.request.input.text =
                    format!("{} {}", leaf.request.input.text, terms.join(" "))
                        .trim()
                        .to_owned();
            }
            leaf.selectors.push(BoundSelector {
                selector: *selector,
                canonical,
                terms,
                targets,
            });
        }
        self.policy.admit(&leaf.request)?;
        Ok(())
    }
    async fn validate_exact(
        &mut self,
        target: &RetrievalTarget,
        leaf: &PlannedQuery,
    ) -> Result<()> {
        let options = nous_storage::QuerySnapshotOptions {
            space_id: "",
            as_of_commit: leaf.request.as_of_commit,
            limit: 1,
            source_read: Some(&self.source_read),
            filters: &leaf.request.filters,
            history: leaf.request.history,
        };
        self.read
            .require_retrieval_reference(&self.namespace, &self.subject, &options, target)
            .await
    }
}
