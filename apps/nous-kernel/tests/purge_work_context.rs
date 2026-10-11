// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;

use nous_core::{CognitiveRef, OperationId};
use nous_runtime::CreateWorkContextInput;
use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
async fn owner_purge_redacts_dependent_work_context_but_keeps_its_shell() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = test_support::query::subject(&runtime).await;
    let source = observation(&runtime, subject, "context source").await;
    let memory = runtime
        .require_memory()
        .unwrap()
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Sensitive memory represented in an active context",
        ))
        .await
        .unwrap();
    let reference = CognitiveRef::MemoryRevision(memory.revision.header.memory_revision_id());
    let context = runtime
        .cognition
        .create_work_context(CreateWorkContextInput {
            operation_id: OperationId::new(),
            subject,
            purpose: "Sensitive active task".into(),
            unresolved_questions: vec!["Is the private detail still true?".into()],
            constraints: serde_json::json!({"confidential": true}),
            resume_conditions: vec!["Resume after disclosure".into()],
            budget_summary: serde_json::json!({"private": "value"}),
            cognition_anchors: vec![reference],
            context_text: "Copied sensitive memory text".into(),
            entity_anchors: vec![],
            tag_anchors: vec![],
        })
        .await
        .unwrap();

    runtime
        .purge_cognition(
            subject,
            CognitiveRef::Memory(memory.header.memory_id()),
            OperationId::new(),
            memory.header.object_epoch,
        )
        .await
        .unwrap();

    let context = runtime
        .cognition
        .work_context(subject, context.work_context_id)
        .await
        .unwrap();
    assert_eq!(context.state, nous_runtime::WorkContextState::Ended);
    assert_eq!(context.purpose, "Purged context");
    assert!(context.context_text.is_empty());
    assert!(context.unresolved_questions.is_empty());
    assert!(context.resume_conditions.is_empty());
    assert_eq!(context.constraints, serde_json::json!({}));
    assert_eq!(context.budget_summary, serde_json::json!({}));
    assert!(context.cognition_anchors.is_empty());
}
