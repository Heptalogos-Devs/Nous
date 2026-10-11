use super::*;

#[expect(
    clippy::too_many_lines,
    reason = "one model query scenario verifies Host material reuse, catalog membership and capability policy"
)]
pub(crate) async fn check_model_catalog_activity(
    rt: &NousRuntime,
    subject: SubjectId,
    tag: TagId,
    probe: &Probe,
) {
    use nous_kernel::transport::KernelService;
    use nous_protocol::nous::wave::kernel::v1alpha1::{
        self as k, kernel_query_service_server::KernelQueryService,
    };
    rt.configuration
        .set_subject_override(
            OperationId::new(),
            subject,
            CONCEPT_ENRICHMENT.path(),
            serde_json::json!("model"),
            None,
        )
        .await
        .unwrap();
    let service = KernelService(rt.clone());
    let before = rt.store.authority_seq(subject).await.unwrap();
    let mut input = query(
        subject,
        vec![Cue::Text(TextCue {
            text: "reader reclamation".into(),
        })],
    );
    input.capabilities.query_concept_enrichment = RequirementStrength::Required;
    let activate = |token: String| k::KernelQueryRequest {
        subject_id: subject.0.to_string(),
        preparation_token: token,
        ..Default::default()
    };
    let bound = rt.cognition.bind_query(input.clone()).await.unwrap();
    let materials = vec![k::QueryEmbedding {
        text: bound.representation.text.clone(),
        space_hash: probe.space().space_hash,
        producer_hash: probe.producer().signature_hash,
        vector: vec![1.0, 0.0],
    }];
    let queries = probe
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(query, _)| *query)
        .count();
    let token = rt
        .cognition
        .retain_prepared_query(test_support::query_reservation(bound))
        .unwrap();
    let activation = service
        .activate_query(tonic::Request::new(k::KernelQueryRequest {
            embeddings: materials,
            ..activate(token.to_string())
        }))
        .await
        .unwrap()
        .into_inner();
    let model_input: serde_json::Value = serde_json::from_str(&activation.model_input).unwrap();
    let catalog = model_input["existing_tags"].as_array().unwrap();
    assert!(
        catalog
            .iter()
            .all(|candidate| candidate.get("tag").is_none())
    );
    let key = catalog[0]["key"].as_str().unwrap();
    let response = service.query(tonic::Request::new(k::KernelQueryRequest {
        concept_model_calls:1,
        concept_output:Some(serde_json::json!({"existing_tags":[{"key":key,"strength":1.0}],"novel_concepts":[{"text":"lease-aware historical cache"}]}).to_string()),
        ..activate(activation.preparation_token)
    })).await.unwrap().into_inner().response.unwrap();
    assert_eq!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(query, _)| *query)
            .count(),
        queries
    );
    rt.cognition
        .use_feedback(nous_runtime::UseFeedback {
            subject,
            session_id: None,
            consumer_ref: "consumer:host:concept".into(),
            events: vec![nous_runtime::UseFeedbackEvent {
                query_id: Some(response.query_id.parse().unwrap()),
                event_id: UseEventId::new(),
                reference: response
                    .hits
                    .first()
                    .unwrap()
                    .reference
                    .as_ref()
                    .map(|reference| parse_reference(&reference.kind, &reference.value).unwrap())
                    .unwrap(),
                use_kind: nous_runtime::UseKind::ResultSupported,
                occurred_at: rt.cognition.now(subject),
                context: serde_json::json!({}),
            }],
        })
        .await
        .unwrap();
    let linked = nous_runtime::linked_query_feedback(
        &rt.store,
        subject,
        &parse_reference(
            &response.hits[0].reference.as_ref().unwrap().kind,
            &response.hits[0].reference.as_ref().unwrap().value,
        )
        .unwrap(),
        rt.cognition.now(subject),
    )
    .await
    .unwrap();
    assert_eq!(
        linked[0].signals.novel_concepts[0].text,
        "lease-aware historical cache"
    );
    let bound: serde_json::Value =
        serde_json::from_str(response.bound_query.as_deref().unwrap()).unwrap();
    assert_eq!(bound["query_activation"]["model_calls"], 1);
    assert_eq!(
        bound["query_activation"]["novel_concepts"][0]["text"],
        "lease-aware historical cache"
    );
    assert!(
        bound["query_activation"]["inferred_tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source"] == "model_inferred"
                && item["tag"] == serde_json::json!(tag))
    );
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);
    let token = rt
        .cognition
        .retain_prepared_query(test_support::query_reservation(
            rt.cognition.bind_query(input.clone()).await.unwrap(),
        ))
        .unwrap();
    let activation = service
        .activate_query(tonic::Request::new(activate(token.to_string())))
        .await
        .unwrap()
        .into_inner();
    let error = service.query(tonic::Request::new(k::KernelQueryRequest {concept_model_calls:1,concept_output:Some(serde_json::json!({"existing_tags":[{"key":"c99","strength":1.0}],"novel_concepts":[]}).to_string()),..activate(activation.preparation_token)})).await.unwrap_err();
    assert_eq!(error.code(), tonic::Code::InvalidArgument);
    assert!(error.message().contains("outside frozen catalog"));
    assert!(matches!(
        rt.execute_query(input.clone(), None).await,
        Err(Error::Unavailable(_))
    ));
    input.capabilities.query_concept_enrichment = RequirementStrength::Forbidden;
    let token = rt
        .cognition
        .retain_prepared_query(test_support::query_reservation(
            rt.cognition.bind_query(input.clone()).await.unwrap(),
        ))
        .unwrap();
    assert_eq!(
        service
            .activate_query(tonic::Request::new(activate(token.to_string())))
            .await
            .unwrap_err()
            .code(),
        tonic::Code::InvalidArgument
    );
    let disabled = rt.execute_query(input, None).await.unwrap();
    assert_eq!(disabled.bound.activation.model_calls, 0);
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);
}
