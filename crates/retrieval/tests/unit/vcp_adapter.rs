// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
use nous_core::{MemoryRevisionId, TagId};

#[test]
fn identity_curve_evidence_and_readout_mapping_preserve_generation_contracts() {
    let body = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
    let first = CognitiveRef::Tag(TagId::new());
    let second = CognitiveRef::Tag(TagId::new());
    let ids = VcpIdentityMap::new([body.clone(), first.clone(), second.clone(), first.clone()]);
    let reversed = VcpIdentityMap::new([second.clone(), first.clone(), body.clone()]);
    assert_eq!(ids.references(), reversed.references());
    for r in ids.references() {
        assert_eq!(ids.reference(ids.id(r).unwrap()).unwrap(), r);
    }
    for id in [0, -1, i64::MIN, 4] {
        assert!(ids.reference(id).is_err());
    }
    let mut source = VcpCurveSource {
        reference: body.clone(),
        vector: vec![1.0, 0.0],
        members: vec![
            VcpCurveMember {
                reference: second.clone(),
                vector: vec![0.0, 1.0],
            },
            VcpCurveMember {
                reference: first.clone(),
                vector: vec![1.0, 0.0],
            },
            VcpCurveMember {
                reference: second.clone(),
                vector: vec![0.0, 1.0],
            },
        ],
        order: VcpCurveOrder::SourceSequence,
    };
    let curve = vcp_candidate_curve(&ids, &source).unwrap();
    assert_eq!(curve.id, ids.id(&body).unwrap());
    assert_eq!(
        curve.tags.iter().map(|t| t.id).collect::<Vec<_>>(),
        vec![ids.id(&second).unwrap(), ids.id(&first).unwrap()]
    );
    assert_eq!(
        curve.tags.iter().map(|t| t.position).collect::<Vec<_>>(),
        vec![1, 2]
    );
    source.order = VcpCurveOrder::StableIdentity;
    let curve = vcp_candidate_curve(&ids, &source).unwrap();
    assert!(curve.tags[0].id < curve.tags[1].id);
    source.members[0].vector = vec![1.0];
    assert!(vcp_candidate_curve(&ids, &source).is_err());
    let edge = WaveEdgeEvidence {
        from: first.clone(),
        to: second.clone(),
        basis_class: "source_evidence".into(),
        association_kind: "basis".into(),
        polarity: "positive".into(),
        support_mass: 0.6,
        provenance_root: Some("occurrence:one".into()),
    };
    let mut stronger = edge.clone();
    stronger.support_mass = 0.9;
    let mut independent = edge.clone();
    independent.provenance_root = Some("occurrence:two".into());
    let mut negative = edge.clone();
    negative.polarity = "negative".into();
    let values =
        vcp_evidence_contributions(&ids, &[edge, stronger, independent, negative]).unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values[0].support_mass, 0.9);
    assert_eq!(values[1].support_mass, 0.6);
    assert_eq!(values[0].basis_class, "source_evidence");
    let metadata = serde_json::json!({"final_score":0.9});
    let ranked = vec![
        (ids.id(&body).unwrap(), metadata.clone()),
        (ids.id(&body).unwrap(), serde_json::Value::Null),
        (ids.id(&first).unwrap(), serde_json::Value::Null),
    ];
    let mapped = vcp_lane_candidates(&ids, &ranked, "vcp-rivermemo-v3.1-adapter-v1", 2).unwrap();
    assert_eq!(mapped.len(), 2);
    assert_eq!(mapped[0].reference, body);
    assert_eq!(mapped[0].provider_metadata, metadata);
    assert_eq!(mapped[1].rank, 2);
    assert!(vcp_lane_candidates(&ids, &[(99, serde_json::Value::Null)], "profile", 1).is_err());
}
