use super::*;

pub(crate) async fn check_permission_fence(
    rt: &nous_kernel::NousRuntime,
    subject: SubjectId,
    view: &HistoricalAuthoritySnapshot,
) {
    let reference = view.cognition[0].head.clone();
    let query = CognitiveQuery {
        subject,
        session: None,
        work_context: None,
        projection: Default::default(),
        temporal_frame: TemporalFrame {
            authority_view: AuthorityView::AsOf(view.as_of),
            ..Default::default()
        },

        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "read historical cognition".into(),
            })],
            targets: vec![QueryTarget::Exact {
                reference: reference.clone(),
            }],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    };
    let bound = rt
        .bind_query_with_snapshot(
            query,
            rt.configuration.snapshot_for_subject(subject).unwrap(),
        )
        .await
        .unwrap();
    sqlx::query("UPDATE subject_capabilities SET memory=false WHERE subject_id=$1")
        .bind(subject.0)
        .execute(rt.store.pool())
        .await
        .unwrap();
    let (hits, drops) = nous_runtime::CognitiveContributor::validate_and_materialize(
        rt.require_memory().unwrap(),
        subject,
        &[reference],
        &bound,
    )
    .await
    .unwrap();
    assert!(hits.is_empty());
    assert!(matches!(
        rt.store
            .bind_reference_in_view(subject, &view.cognition[0].head, Some(view))
            .await,
        Err(Error::NotFound(_))
    ));
    assert!(matches!(
        rt.store
            .bind_reference_in_view(subject, &CognitiveRef::Tag(view.tags[0].tag), Some(view))
            .await,
        Err(Error::NotFound(_))
    ));
    assert_eq!(drops["current_permission_denied"], 1);
    let projected = rt
        .store
        .historical_projection_input(
            view,
            rt.configuration
                .snapshot_for_subject(subject)
                .unwrap()
                .get(nous_configuration::ConfigKey::new("episode.synopsis"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(projected.concepts.tags.is_empty());
    assert!(
        !projected
            .sources
            .iter()
            .any(|source| matches!(source.reference, CognitiveRef::MemoryRevision(_)))
    );
    assert!(
        rt.historical_authority_view(subject, view.as_of, RevisionView::Current)
            .await
            .unwrap()
            .cognition
            .is_empty()
    );
    sqlx::query("UPDATE subject_capabilities SET memory=true WHERE subject_id=$1")
        .bind(subject.0)
        .execute(rt.store.pool())
        .await
        .unwrap();
}

pub(crate) async fn check_historical_identity(
    rt: &nous_kernel::NousRuntime,
    view: &HistoricalAuthoritySnapshot,
    tag: TagId,
) {
    assert_eq!(
        rt.store
            .resolve_identity(view.subject, "tag", "Future concept", false)
            .await
            .unwrap()
            .0,
        "BOUND"
    );
    let (status, identities) = rt
        .store
        .resolve_identity_in_view(view, "tag", "Past concept", false)
        .await
        .unwrap();
    assert_eq!(status, "BOUND");
    assert_eq!(identities[0].canonical, CognitiveRef::Tag(tag));
    assert_eq!(
        rt.store
            .resolve_identity_in_view(view, "tag", "Future concept", false)
            .await
            .unwrap()
            .0,
        "UNKNOWN_REFERENCE"
    );
}
