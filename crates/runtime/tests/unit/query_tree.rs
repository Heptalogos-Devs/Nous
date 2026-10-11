// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn branch_allocation_preserves_total_and_scopes_cannot_broaden_filters() {
    for count in 1..65 {
        assert_eq!(
            (0..count)
                .map(|index| allocation(48, index, count))
                .sum::<usize>(),
            48
        );
    }
    let parent = QueryConstraints {
        current_authority: CurrentAuthorityNeed::Required,
        source_classes_include: vec![SourceClass::from("web".to_owned())],
        ..Default::default()
    };
    let child = QueryConstraints {
        source_classes_include: vec![SourceClass::from("file".to_owned())],
        ..Default::default()
    };
    assert!(constraints(&parent, &child).is_none());
    assert_eq!(
        constraints(&parent, &QueryConstraints::default())
            .unwrap()
            .source_classes_include,
        parent.source_classes_include
    );
    assert_eq!(
        constraints(&parent, &QueryConstraints::default())
            .unwrap()
            .current_authority,
        CurrentAuthorityNeed::Required
    );
    assert_eq!(
        constraints(
            &QueryConstraints::default(),
            &QueryConstraints {
                current_authority: CurrentAuthorityNeed::Required,
                ..Default::default()
            }
        )
        .unwrap()
        .current_authority,
        CurrentAuthorityNeed::Required
    );
    let exact = QueryTarget::Exact {
        reference: CognitiveRef::Artifact(ArtifactId::new()),
    };
    assert_eq!(inherit_targets(&[], std::slice::from_ref(&exact)).len(), 1);
}
