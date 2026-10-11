use super::*;

pub(crate) async fn check_activation_configuration(rt: &NousRuntime, subject: SubjectId) {
    for label in ["matching policy probe", "orthogonal policy probe"] {
        rt.require_memory()
            .unwrap()
            .create_tag(
                subject,
                CreateTagRequest {
                    operation_id: OperationId::new(),
                    label: label.into(),
                    description: None,
                    kind_hint: None,
                    origin: "host_explicit".into(),
                    producer: None,
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
