// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

#[test]
fn source_coordinates_are_not_parser_chunk_ordinals() {
    let region = SourceRegion {
        source_region_id: SourceRegionId::new(),
        subject_id: SubjectId::new(),
        artifact_id: ArtifactId::new(),
        coordinate_kind: "text_span".into(),
        coordinate: serde_json::json!({"encoding":"utf-8","start":0,"end":4}),
        coordinate_hash: "stable-coordinate".into(),
        parent_source_region_id: None,
        created_at: Utc::now(),
    };
    assert!(region.validate().is_ok());
    assert!(
        SourceRegion {
            coordinate_kind: "chunk_12".into(),
            ..region
        }
        .validate()
        .is_err()
    );
}

#[test]
fn temporal_extent_preserves_unknown_and_half_open_intervals() {
    assert!(TemporalExtent::Unknown.validate().is_ok());
    assert!(
        TemporalExtent::Interval {
            start: Some(Utc::now()),
            end: Some(Utc::now() + chrono::Duration::seconds(1)),
        }
        .validate()
        .is_ok()
    );
}
