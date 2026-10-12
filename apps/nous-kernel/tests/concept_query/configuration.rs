use super::*;

pub(crate) async fn check_activation_configuration(rt: &NousRuntime, subject: SubjectId) {
    for label in ["matching policy probe", "orthogonal policy probe"] {
        rt.require_memory()
            .unwrap()
            .create_tag(
                subject,
                CreateTagRequest {
                    producer: None,
                    operation_id: OperationId::new(),
                    content: nous_memory::TagContent {
                        label: label.into(),
                        description: None,
                        kind_hint: None,
                    },
                    origin: "host_explicit".into(),
                },
            )
            .await
            .unwrap();
    }
    let limit = "retrieval.query.concept_activation.max_activated_tags";
    let catalog = "retrieval.query.concept_activation.model_catalog_limit";
    let threshold = "retrieval.query.concept_activation.minimum_similarity";
    for (path, value) in [
        (limit, serde_json::json!(1)),
        (catalog, serde_json::json!(1)),
        (threshold, serde_json::json!(0.0)),
    ] {
        rt.configuration
            .set_subject_override(OperationId::new(), subject, path, value, None)
            .await
            .unwrap();
    }
    let input = query(subject, vec![]);
    let frozen = rt.cognition.bind_query(input.clone()).await.unwrap();
    for path in [limit, catalog] {
        rt.configuration
            .set_subject_override(
                OperationId::new(),
                subject,
                path,
                serde_json::json!(3),
                None,
            )
            .await
            .unwrap();
    }
    let old = rt.execute_bound_query(frozen, None).await.unwrap();
    assert_eq!(old.bound.activation.inferred_tags.len(), 1);
    assert_eq!(old.bound.activation.concept_catalog.len(), 1);
    let new = rt.execute_query(input.clone(), None).await.unwrap();
    assert_eq!(new.bound.activation.inferred_tags.len(), 3);
    assert_eq!(new.bound.activation.concept_catalog.len(), 3);
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            threshold,
            serde_json::json!(0.5),
            None,
        )
        .await
        .unwrap();
    let filtered = rt.execute_query(input, None).await.unwrap();
    assert_eq!(filtered.bound.activation.inferred_tags.len(), 2);
    assert_eq!(filtered.bound.activation.concept_catalog.len(), 3);
}

// Both VCP readouts share text/vector assets; native consumes structure alone.
#[expect(
    clippy::too_many_lines,
    reason = "one query scenario checks native and VCP invalidation and asset reuse"
)]
pub(crate) async fn check_topology_asset_dependencies(rt: &NousRuntime, subject: SubjectId) {
    let need = ServingNeed {
        topology: true,
        concept_vectors: true,
        ..Default::default()
    };
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            COGNITIVE_PROFILE.path(),
            serde_json::json!("nous-node-potential-v1"),
            None,
        )
        .await
        .unwrap();
    rt.serving.prepare(subject, need).await.unwrap();
    let native = rt
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "topology" && record.space.is_empty())
        .unwrap();
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            COGNITIVE_PROFILE.path(),
            serde_json::json!("vcp-dtsc-v9.2.1-adapter-v1"),
            None,
        )
        .await
        .unwrap();
    rt.serving.prepare(subject, need).await.unwrap();
    let vcp = rt
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "topology" && record.space == "vcp:concept-test")
        .unwrap();
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            COGNITIVE_PROFILE.path(),
            serde_json::json!("vcp-rivermemo-v3.1-adapter-v1"),
            None,
        )
        .await
        .unwrap();
    let shared = rt.serving.prepare(subject, need).await.unwrap();
    assert_eq!(
        shared.generations["topology:vcp:concept-test"],
        vcp.generation_id
    );
    assert!(
        !shared
            .rebuilt
            .contains(&"topology:vcp:concept-test".to_owned())
    );

    let added = observation(
        rt,
        subject,
        "A new text source updates VCP vectors without changing native structure",
    )
    .await;
    let families = rt.serving.family_status(subject).await.unwrap();
    let status = families
        .iter()
        .find(|family| family.name == "topology" && family.space_signature == vcp.space)
        .unwrap();
    assert_eq!(status.state, nous_runtime::LaneStatus::Stale);
    assert!(status.desired_authority_seq > vcp.authority_watermark);
    let native_status = families
        .iter()
        .find(|family| family.name == "topology" && family.space_signature.is_empty())
        .unwrap();
    assert_eq!(native_status.state, nous_runtime::LaneStatus::Ready);
    assert!(
        !rt.store
            .promote_generation(vcp.generation_id)
            .await
            .unwrap()
    );
    let updated = rt.serving.prepare(subject, need).await.unwrap();
    assert!(
        updated
            .rebuilt
            .contains(&"topology:vcp:concept-test".to_owned())
    );
    let record = rt
        .store
        .serving_current(subject)
        .await
        .unwrap()
        .into_iter()
        .find(|record| record.family == "topology" && record.space == "vcp:concept-test")
        .unwrap();
    let generation: nous_retrieval::VcpGeneration = serde_json::from_slice(
        &std::fs::read(
            rt.serving
                .artifact_directory(record.generation_id)
                .join("vcp.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        generation.identities.references().iter().any(
            |reference| reference == &CognitiveRef::Occurrence(added.occurrence.occurrence_id)
        )
    );

    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            COGNITIVE_PROFILE.path(),
            serde_json::json!("nous-node-potential-v1"),
            None,
        )
        .await
        .unwrap();
    let restored = rt.serving.prepare(subject, need).await.unwrap();
    assert_eq!(restored.generations["topology"], native.generation_id);
    assert!(!restored.rebuilt.contains(&"topology".to_owned()));
}
