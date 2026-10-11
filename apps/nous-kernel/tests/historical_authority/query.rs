use super::*;

pub(crate) async fn check_historical_binding(
    rt: &nous_kernel::NousRuntime,
    subject: SubjectId,
    view: &HistoricalAuthoritySnapshot,
    memory: MemoryId,
    references: (CognitiveRef, CognitiveRef),
    tags: (TagId, TagId),
) {
    let (old, future) = references;
    let (tag, future_tag) = tags;
    let context = rt
        .cognition
        .create_work_context(nous_runtime::CreateWorkContextInput {
            operation_id: OperationId::new(),
            subject,
            purpose: "CURRENT_QUESTION PURPOSE about old knowledge".into(),
            unresolved_questions: vec![],
            constraints: serde_json::json!({}),
            resume_conditions: vec![],
            budget_summary: serde_json::json!({}),
            context_text: String::new(),
            entity_anchors: vec![],
            tag_anchors: vec![],
            cognition_anchors: vec![old.clone(), future.clone()],
        })
        .await
        .unwrap();
    let query = CognitiveQuery {
        subject,
        session: None,
        work_context: Some(context.work_context_id),
        projection: Default::default(),
        temporal_frame: TemporalFrame {
            authority_view: AuthorityView::AsOf(view.as_of),
            ..Default::default()
        },

        situation: SituationDescriptor {
            current_refs: vec![CognitiveRef::Tag(future_tag)],
            ..Default::default()
        },
        expression: CognitiveQueryExpr {
            targets: vec![QueryTarget::Exact {
                reference: CognitiveRef::Memory(memory),
            }],
            cues: vec![
                Cue::Text(TextCue {
                    text: "what was known".into(),
                }),
                Cue::Tag(TagCue { tag }),
            ],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    };
    let config = rt.configuration.snapshot_for_subject(subject).unwrap();
    let bound = rt
        .cognition
        .bind_query_with_authority_view(query.clone(), config.clone(), Some(Arc::new(view.clone())))
        .await
        .unwrap();
    assert_eq!(bound.exact_bindings[0].bound_ref, old);
    check_historical_identity(rt, view, tag).await;
    assert!(bound.runtime_refs.contains(&old));
    assert!(!bound.runtime_refs.contains(&future));
    assert!(!bound.runtime_refs.contains(&CognitiveRef::Tag(future_tag)));
    assert!(bound.representation.text.contains("PAST_DESCRIPTION"));
    assert!(
        bound
            .representation
            .text
            .contains("original authority fact")
    );
    assert!(
        bound
            .representation
            .text
            .contains("CURRENT_QUESTION PURPOSE")
    );
    assert!(!bound.representation.text.contains("FUTURE_DESCRIPTION"));
    assert!(
        !bound
            .representation
            .text
            .contains("rephrased authority fact")
    );
    assert!(
        bound
            .activation
            .degradation
            .iter()
            .any(|degradation| degradation.code == "future_context_ref_excluded")
    );
    assert_eq!(bound.activation.explicit_tags[0].tag, tag);
    let composed = rt
        .bind_query_with_snapshot(query.clone(), config.clone())
        .await
        .unwrap();
    assert!(composed.historical_authority.is_some());
    assert_eq!(composed.exact_bindings[0].bound_ref, old);
    // Binding/reading the exact object uses Authority. Exercise historical
    // Serving with the separate discovery intent and the same as-of context.
    let mut discovery = query.clone();
    discovery.expression.targets.clear();
    let discovery = rt
        .bind_query_with_snapshot(discovery, config.clone())
        .await
        .unwrap();
    check_historical_serving(rt, &discovery, &old, &future, tag).await;
    check_historical_execution(rt, query.clone(), &old, &future).await;
    let mut future_query = query;
    future_query
        .expression
        .cues
        .push(Cue::Tag(TagCue { tag: future_tag }));
    assert!(matches!(
        rt.cognition
            .bind_query_with_authority_view(future_query, config, Some(Arc::new(view.clone())))
            .await,
        Err(Error::NotFound(_))
    ));
}

pub(super) async fn check_historical_execution(
    rt: &nous_kernel::NousRuntime,
    query: CognitiveQuery,
    old: &CognitiveRef,
    future: &CognitiveRef,
) {
    let execution = rt.execute_query(query, None).await.unwrap();
    assert!(
        execution.read_lease.is_none(),
        "exact historical reads do not acquire Serving generations"
    );
    assert!(
        execution
            .result
            .results
            .iter()
            .any(|hit| &hit.reference == old
                && hit.representation.as_deref() == Some("original authority fact"))
    );
    assert!(
        !execution
            .result
            .results
            .iter()
            .any(|hit| &hit.reference == future)
    );
}

pub(super) async fn check_historical_serving(
    rt: &nous_kernel::NousRuntime,
    bound: &nous_runtime::BoundQuery,
    old: &CognitiveRef,
    future: &CognitiveRef,
    tag: TagId,
) {
    let view = bound.historical_authority.as_deref().unwrap();
    let input = rt
        .store
        .historical_projection_input(
            view,
            bound
                .config_snapshot
                .get(nous_configuration::ConfigKey::new("episode.synopsis"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        input
            .sources
            .iter()
            .any(|s| &s.reference == old && s.text.as_deref() == Some("original authority fact"))
    );
    assert!(!input.sources.iter().any(|s| &s.reference == future));
    let concept = input.concepts.tags.iter().find(|t| t.tag == tag).unwrap();
    assert!(concept.semantic.text.contains("PAST_DESCRIPTION"));
    assert!(concept.attachments.contains(old));
    assert!(!concept.attachments.contains(future));
    let plan = nous_runtime::QueryPlan::for_bound_query(bound);
    let (first, reader) = rt.serving.prepare_query(bound, &plan).await.unwrap();
    assert!(first.degradation.is_empty(), "{:?}", first.degradation);
    assert!(first.generations.contains_key("lexical"));
    let activation = nous_runtime::QueryActivationView::provider(reader.as_ref())
        .activate(bound)
        .await
        .unwrap();
    assert!(activation.explicit_tags.iter().any(|t| t.tag == tag));
    let second = rt.serving.prepare_query(bound, &plan).await.unwrap().0;
    assert_eq!(first.generations, second.generations);
    assert!(second.rebuilt.is_empty());
    assert!(
        rt.store
            .serving_current(view.subject)
            .await
            .unwrap()
            .iter()
            .all(|record| !first
                .generations
                .values()
                .any(|id| *id == record.generation_id))
    );
    let collected = rt
        .serving
        .reclaim_retired(view.subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(collected.reclaimed.is_empty());
    drop(reader);
    let collected = rt
        .serving
        .reclaim_retired(view.subject, std::time::Duration::ZERO)
        .await
        .unwrap();
    assert!(
        first
            .generations
            .values()
            .all(|id| collected.reclaimed.contains(id))
    );
}
