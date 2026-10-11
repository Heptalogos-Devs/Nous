// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

async fn execute_with_concept_output(
    rt: &NousRuntime,
    subject: SubjectId,
    tag: TagId,
    key: &str,
) -> nous_core::Result<nous_runtime::QueryExecution> {
    let mut source = query(
        subject,
        vec![Cue::Text(TextCue {
            text: "reader reclamation".into(),
        })],
    );
    source.capabilities.query_concept_enrichment = RequirementStrength::Required;
    let mut bound = rt.cognition.bind_query(source).await?;
    bound.selected_embedding_space = Some(rt.serving.embedding().unwrap().space());
    let plan = nous_runtime::QueryPlan::for_bound_query(&bound);
    let material = nous_retrieval::QueryEmbedding {
        text: bound.representation.text.clone(),
        output: TextEmbeddingOutput {
            vector: vec![1.0, 0.0],
            space: probe_space(rt),
            producer: probe_producer(rt),
        },
    };
    nous_retrieval::with_query_material(vec![material], async {
        use nous_runtime::SharedLaneProvider;
        let (projection, view) = rt.serving.prepare_query(&bound, &plan).await?;
        let activation = view.activate(&bound).await?;
        bound.activation = activation;
        bound.activation.degradation.extend(projection.degradation);
        let candidate_key = if key == "<first>" {
            bound
                .activation
                .concept_catalog
                .iter()
                .find(|candidate| candidate.tag == tag)
                .map(|candidate| candidate.key.clone())
                .ok_or_else(|| {
                    Error::new(
                        ErrorOwner::Runtime,
                        ErrorCode::Unavailable,
                        "Concept catalog does not contain the requested tag",
                    )
                })?
        } else {
            key.to_owned()
        };
        let output = nous_runtime::QueryConceptOutput {
            existing_tags: vec![nous_runtime::QueryConceptSelection {
                key: candidate_key,
                strength: 1.0,
            }],
            novel_concepts: vec![nous_runtime::NovelConceptHypothesis {
                text: "lease-aware historical cache".into(),
            }],
        };
        let policy = nous_runtime::ConceptActivationPolicy::from_snapshot(&bound.config_snapshot)?;
        bound.activation.apply_concept_model(output, &policy)?;
        bound.activation.model_calls = 1;
        bound.activation.frozen = true;
        bound.activation_view = Some(view);
        rt.execute_bound_query_with_plan(bound, plan, None).await
    })
    .await
}

fn probe_space(rt: &NousRuntime) -> nous_core::EmbeddingSpaceSignature {
    rt.serving.embedding().unwrap().space()
}

fn probe_producer(rt: &NousRuntime) -> nous_core::ProducerSignature {
    rt.serving.embedding().unwrap().producer()
}

pub(crate) async fn check_model_catalog_activity(
    rt: &NousRuntime,
    subject: SubjectId,
    tag: TagId,
    probe: &Probe,
) {
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
    let before = rt.store.authority_seq(subject).await.unwrap();
    let queries = probe
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(query, _)| *query)
        .count();
    let execution = execute_with_concept_output(rt, subject, tag, "<first>")
        .await
        .unwrap();
    assert_eq!(
        probe
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(query, _)| *query)
            .count(),
        queries,
        "prepared query vectors are reused across activation and retrieval"
    );
    let result = &execution.result;
    rt.cognition
        .use_feedback(nous_runtime::UseFeedback {
            subject,
            session_id: None,
            consumer_ref: "consumer:host:concept".into(),
            events: vec![nous_runtime::UseFeedbackEvent {
                query_id: Some(result.query_id),
                event_id: UseEventId::new(),
                reference: result.results.first().unwrap().reference.clone(),
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
        &result.results[0].reference,
        rt.cognition.now(subject),
    )
    .await
    .unwrap();
    assert_eq!(
        linked[0].signals.novel_concepts[0].text,
        "lease-aware historical cache"
    );
    assert_eq!(execution.bound.activation.model_calls, 1);
    assert_eq!(
        execution.bound.activation.novel_concepts[0].text,
        "lease-aware historical cache"
    );
    assert!(
        execution
            .bound
            .activation
            .inferred_tags
            .iter()
            .any(
                |item| item.source == nous_runtime::ActivationSource::ModelInferred
                    && item.tag == tag
            )
    );
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);

    let invalid = execute_with_concept_output(rt, subject, tag, "c99").await;
    assert!(
        invalid.is_err(),
        "keys outside the frozen catalog are rejected"
    );
    let forbidden = query(
        subject,
        vec![Cue::Text(TextCue {
            text: "reader reclamation".into(),
        })],
    );
    let mut forbidden = forbidden;
    forbidden.capabilities.query_concept_enrichment = RequirementStrength::Forbidden;
    let disabled = rt.execute_query(forbidden, None).await.unwrap();
    assert_eq!(disabled.bound.activation.model_calls, 0);
    assert_eq!(rt.store.authority_seq(subject).await.unwrap(), before);
}
