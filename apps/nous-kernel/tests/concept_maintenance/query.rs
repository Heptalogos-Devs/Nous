// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

pub(super) async fn assert_maintained_concept_query(
    rt: &NousRuntime,
    subject: SubjectId,
    tag: &CognitiveRef,
    revisions: &[CognitiveRef],
) {
    let graph = rt
        .store
        .topology_projection_input(subject, true)
        .await
        .unwrap();
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.association_kind == "tag_attachment" && &edge.to == tag)
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.association_kind == "assoc.related")
    );
    let tag_id = match tag {
        CognitiveRef::Tag(id) => *id,
        _ => unreachable!(),
    };
    let query = CognitiveQuery {
        subject,
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![
                Cue::Text(nous_core::TextCue {
                    text: "Recall this maintained concept".into(),
                }),
                Cue::Tag(TagCue { tag: tag_id }),
            ],
            ..Default::default()
        },
        exploration: ExplorationIntent::AroundTag,
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: CapabilityPolicy {
            text_embedding: RequirementStrength::Forbidden,
            ..Default::default()
        },
        diagnostics: DiagnosticsRequest::Full,
    };
    let bound = rt
        .cognition
        .bind_query(query)
        .await
        .unwrap()
        .for_profile(CognitiveProfile::NousNodePotential)
        .unwrap();
    let execution = rt.execute_bound_query(bound, Some(32)).await.unwrap();
    assert!(
        execution
            .result
            .diagnostics
            .as_ref()
            .unwrap()
            .trace
            .is_some()
    );
    for hit in &execution.result.results {
        if hit
            .match_evidence
            .families
            .contains(&EvidenceFamily::TopologyWave)
        {
            let readout: serde_json::Value =
                serde_json::from_str(hit.match_evidence.explanation.as_deref().unwrap()).unwrap();
            assert!(readout["topologywave"]["activated_route"].is_array());
            for edge in readout["topologywave"]["route_evidence"]
                .as_array()
                .unwrap()
            {
                let basis = edge["support"].as_array().unwrap();
                assert!(!basis.is_empty());
                assert!(basis.iter().all(|item| item["provenance_root"].is_string()));
            }
        }
    }
    for revision in revisions {
        assert!(
            execution
                .result
                .results
                .iter()
                .any(|hit| hit.revision.as_ref() == Some(revision) || &hit.reference == revision),
            "{:#?}",
            execution.result
        );
    }
    assert!(execution.result.results.iter().any(|hit| {
        hit.match_evidence
            .families
            .contains(&EvidenceFamily::TopologyWave)
    }));
    drop(execution);
}
