use super::*;

pub(crate) async fn check_historical_profiles(
    rt: &NousRuntime,
    subject: SubjectId,
    tag: TagId,
    revision: &CognitiveRef,
    cut: chrono::DateTime<chrono::Utc>,
) {
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            CONCEPT_ENRICHMENT.path(),
            serde_json::json!("existing"),
            None,
        )
        .await
        .unwrap();
    for profile in [
        "nous-node-potential-v1",
        "vcp-dtsc-v9.2.1-adapter-v1",
        "vcp-rivermemo-v3.1-adapter-v1",
    ] {
        rt.configuration
            .set_subject_override(
                OperationId::new(),
                subject,
                COGNITIVE_PROFILE.path(),
                serde_json::json!(profile),
                None,
            )
            .await
            .unwrap();
        let mut query = query(
            subject,
            vec![
                Cue::Text(TextCue {
                    text: "reader reclamation".into(),
                }),
                Cue::Tag(TagCue { tag }),
            ],
        );
        query.temporal_frame.authority_view = AuthorityView::AsOf(cut);
        query.exploration = ExplorationIntent::BoundedAssociative;
        let result = rt.execute_query(query, None).await.unwrap();
        assert!(
            result
                .result
                .results
                .iter()
                .any(|hit| &hit.reference == revision)
        );
        let records = rt
            .store
            .historical_serving_reusable(
                subject,
                &result
                    .bound
                    .historical_authority
                    .as_ref()
                    .unwrap()
                    .snapshot_digest,
            )
            .await
            .unwrap();
        assert!(records.iter().any(|r| r.family == "dense"));
        assert!(records.iter().any(|r| r.family == "topology"));
        let concepts = records
            .iter()
            .find(|r| r.family == "concept" && !r.space.is_empty())
            .unwrap();
        let content =
            std::fs::read(std::path::Path::new(&concepts.artifact_location).join("concept.json"))
                .unwrap();
        let generation: nous_retrieval::ConceptGeneration =
            serde_json::from_slice(&content).unwrap();
        let record = generation.record(tag).unwrap();
        assert!(record.semantic.text.contains("Wait for all active readers"));
        assert!(!record.semantic.text.contains("historical assets"));
        assert!(record.vector.is_some());
        check_historical_embedding(
            rt,
            subject,
            result.bound.historical_authority.as_deref().unwrap(),
            &generation,
            tag,
        )
        .await;
        let (_, reader) = rt
            .serving
            .prepare_query(
                &result.bound,
                &nous_runtime::QueryPlan::for_bound_query(&result.bound),
            )
            .await
            .unwrap();
        let lanes = nous_runtime::QueryActivationView::provider(reader.as_ref())
            .lanes(
                &result.bound,
                &nous_runtime::QueryPlan::for_bound_query(&result.bound),
            )
            .await
            .unwrap();
        for family in [EvidenceFamily::Dense, EvidenceFamily::TopologyWave] {
            let lane = lanes.iter().find(|lane| lane.family == family).unwrap();
            assert!(
                !matches!(lane.status, nous_runtime::LaneStatus::Unavailable),
                "{profile}: {:?}",
                lane.diagnostics
            );
        }
    }
}

pub(super) async fn check_historical_embedding(
    rt: &NousRuntime,
    subject: SubjectId,
    view: &HistoricalAuthoritySnapshot,
    generation: &nous_retrieval::ConceptGeneration,
    tag: TagId,
) {
    let needs = rt
        .serving
        .embedding_needs_page(subject, 256, None, Some(view))
        .await
        .unwrap()
        .needs;
    assert!(
        needs
            .iter()
            .all(|need| !need.text.contains("Retain immutable historical assets"))
    );
    if let Some(need) = needs
        .iter()
        .find(|need| need.reference == CognitiveRef::Tag(tag))
    {
        assert!(
            rt.serving
                .commit_embedding(
                    subject,
                    need.reference.clone(),
                    need.text.clone(),
                    &generation.space.as_ref().unwrap().space_hash,
                    &generation.producer.as_ref().unwrap().signature_hash,
                    vec![1.0, 0.0]
                )
                .await
                .is_err()
        );
        rt.serving
            .commit_embedding_in_view(
                subject,
                need.reference.clone(),
                need.text.clone(),
                &generation.space.as_ref().unwrap().space_hash,
                &generation.producer.as_ref().unwrap().signature_hash,
                vec![1.0, 0.0],
                Some(view),
            )
            .await
            .unwrap();
    }
}
