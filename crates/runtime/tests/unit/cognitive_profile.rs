// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
#[test]
fn fixed_registry_rejects_dynamic_names_and_describes_requirements() {
    for profile in CognitiveProfile::ALL {
        let encoded = serde_json::to_value(profile).expect("profile value");
        assert_eq!(encoded.as_str(), Some(profile.id()));
        assert_eq!(
            serde_json::from_value::<CognitiveProfile>(encoded).expect("profile"),
            profile
        );
    }
    assert!(
        serde_json::from_value::<CognitiveProfile>(serde_json::json!("arbitrary-plugin")).is_err()
    );
    assert!(!CognitiveProfile::BaselineRrf.requirements().topology);
    assert!(
        !CognitiveProfile::NousNodePotential
            .requirements()
            .query_embedding
    );
    assert!(
        CognitiveProfile::VcpRiverMemo
            .requirements()
            .query_embedding
    );
}
