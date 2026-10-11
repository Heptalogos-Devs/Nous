// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;
fn query() -> CognitiveQuery {
    CognitiveQuery {
        subject: SubjectId::new(),
        projection: Default::default(),
        temporal_frame: Default::default(),

        work_context: None,
        session: None,
        situation: Default::default(),
        expression: CognitiveQueryExpr {
            cues: vec![Cue::Text(TextCue {
                text: "Alice develops Nous Wave".into(),
            })],
            ..Default::default()
        },
        exploration: Default::default(),
        resources: Default::default(),
        result_need: Default::default(),
        effort: Default::default(),
        capabilities: Default::default(),
        diagnostics: Default::default(),
    }
}
#[test]
fn explicit_and_pinned_descriptors_precede_work_and_resident_background() {
    let query = query();
    let tag = CognitiveRef::Tag(TagId::new());
    let exact = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
    let mut descriptors = vec![
        QueryDescriptor {
            reference: tag.clone(),
            text: "Pinned blogging practice".into(),
        },
        QueryDescriptor {
            reference: exact.clone(),
            text: "Explicit selected revision".into(),
        },
    ];
    let mut sources = vec![
        (tag, "work_context".into()),
        (exact, "explicit_exact".into()),
    ];
    for index in 0..16 {
        let reference = CognitiveRef::MemoryRevision(MemoryRevisionId::new());
        descriptors.push(QueryDescriptor {
            reference: reference.clone(),
            text: format!("Resident {index}: {}", "background ".repeat(60)),
        });
        sources.push((reference, "runtime_resident".into()));
    }
    let now = chrono::Utc::now();
    let context = WorkContextView {
        work_context_id: uuid::Uuid::new_v4(),
        subject_id: query.subject,
        state: WorkContextState::Open,
        purpose: "Investigate blogging".into(),
        context_text: format!(
            "Simon Willison link blog beats task context {}",
            "task background ".repeat(600)
        ),
        unresolved_questions: vec!["Why did the practice change?".into()],
        constraints: serde_json::json!({}),
        resume_conditions: vec![],
        budget_summary: serde_json::json!({}),
        revision: 1,
        created_at: now,
        updated_at: now,
        ended_at: None,
        cognition_anchors: vec![],
        entity_anchors: vec![],
        tag_anchors: vec![],
    };
    let representation = build_query_representation(
        &query,
        &descriptors,
        Some(context),
        sources,
        &QueryRepresentationLimits::default(),
    );
    assert!(representation.text.contains("Alice develops Nous Wave"));
    assert!(representation.text.contains("Pinned blogging practice"));
    assert!(representation.text.contains("Explicit selected revision"));
    assert!(
        representation
            .text
            .contains("Simon Willison link blog beats task context")
    );
    assert!(representation.text.contains("Why did the practice change?"));
    assert!(
        representation
            .truncation_flags
            .contains(&"Current cognition:runtime_background".into())
    );
    assert_eq!(representation.text.matches("Current cognition:").count(), 1);
    assert!(representation.text.chars().count() <= 8192);
}
#[test]
fn allocation_preserves_context_priority_and_fixed_section_order() {
    let mut builder = Builder {
        sections: Vec::new(),
        remaining: 128,
        flags: BTreeSet::new(),
    };
    builder.section("Intent", "Alice reviews deployment readiness", 2048);
    builder.section("Resources", &"r".repeat(128), 512);
    builder.section("Current work", "Review Tide approval", 1024);
    let text = builder.render();
    assert!(text.contains("Review Tide approval"));
    assert!(builder.flags.contains("Resources"));
    assert!(text.find("Resources").unwrap() < text.find("Current work").unwrap());
    assert!(text.chars().count() <= 128);
    assert!(!text.contains("r".repeat(128).as_str()));
}

#[test]
fn representation_is_semantic_deterministic_and_bounded() {
    let mut query = query();
    let a = QueryDescriptor {
        reference: CognitiveRef::Tag(TagId::new()),
        text: "Nous Wave: cognition project".into(),
    };
    let b = QueryDescriptor {
        reference: CognitiveRef::Entity(EntityRef::new("entity:opaque").unwrap()),
        text: "Alice (A)".into(),
    };
    let limits = QueryRepresentationLimits::default();
    let first = build_query_representation(&query, &[a.clone(), b.clone()], None, vec![], &limits);
    let reversed = build_query_representation(&query, &[b, a], None, vec![], &limits);
    assert_eq!(first.text, reversed.text);
    assert_eq!(first.sha256, reversed.sha256);
    assert!(first.text.contains("Alice (A)"));
    assert!(!first.text.contains("entity:opaque"));
    query
        .situation
        .object_descriptions
        .insert("object:opaque".into(), "Nous Wave issue triage".into());
    let changed = build_query_representation(&query, &[], None, vec![], &limits);
    assert!(changed.text.contains("Nous Wave issue triage"));
    assert!(!changed.text.contains("object:opaque"));
    assert_ne!(changed.sha256, first.sha256);
    query.expression.cues = vec![Cue::Text(TextCue {
        text: "a".repeat(10000),
    })];
    let bounded = build_query_representation(&query, &[], None, vec![], &limits);
    assert!(bounded.text.chars().count() <= limits.total_chars);
    assert!(bounded.truncation_flags.contains(&"Intent".into()));
    query.expression.cues = vec![Cue::Text(TextCue {
        text: "What did I discuss yesterday?".into(),
    })];
    let raw = build_query_representation(&query, &[], None, vec![], &limits);
    assert_eq!(
        raw.text,
        "Intent:\nWhat did I discuss yesterday?\n\nCurrent objects:\nNous Wave issue triage"
    );
    assert_eq!(raw.representation_version, "cognitive-query-v2");
}
