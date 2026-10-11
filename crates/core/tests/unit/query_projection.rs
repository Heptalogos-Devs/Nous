// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn default_projection_admits_only_cognition_even_for_exact_refs() {
    let projection = ResultProjection::default();
    for reference in [
        CognitiveRef::MemoryRevision(MemoryRevisionId::new()),
        CognitiveRef::CognitiveSchemaRevision(CognitiveSchemaRevisionId::new()),
        CognitiveRef::EpisodeRevision(EpisodeRevisionId::new()),
        CognitiveRef::JournalRevision(JournalRevisionId::new()),
    ] {
        assert!(projection.allows_reference(&reference));
    }
    assert!(!projection.allows_reference(&CognitiveRef::Artifact(ArtifactId::new())));
    assert!(!projection.allows_reference(&CognitiveRef::Resource(
        ResourceRef::new("resource:test").unwrap()
    )));
    let evidence = ResultProjection {
        domains: vec![ResultDomain::Evidence],
    };
    assert!(evidence.allows_reference(&CognitiveRef::Artifact(ArtifactId::new())));
    assert!(!evidence.allows_reference(&CognitiveRef::MemoryRevision(MemoryRevisionId::new())));
    assert!(ResultProjection { domains: vec![] }.validate().is_err());
    assert!(
        ResultProjection {
            domains: vec![ResultDomain::Memory, ResultDomain::Memory]
        }
        .validate()
        .is_err()
    );
}
