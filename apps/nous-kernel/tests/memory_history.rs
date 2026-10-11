// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;
use nous_core::{OperationId, TemporalExtent};
use nous_memory::{CreateTagRequest, ReviseMemoryInput};
use nous_protocol::public as p;
use p::memory_service_server::MemoryService;
use test_support::query::subject;
use test_support::{database, form_input, observation, open_runtime};

#[tokio::test]
async fn revision_tags_survive_replay_and_history_returns_complete_pages() {
    let (root, url, _postgres) = database().await;
    let runtime = open_runtime(&url, &root).await;
    let subject = subject(&runtime).await;
    let source = observation(&runtime, subject, "Research note").await;
    let owner = runtime.require_memory().unwrap();
    let tag = owner
        .create_tag(
            subject,
            CreateTagRequest {
                operation_id: OperationId::new(),
                producer: None,
                label: "Research".into(),
                description: None,
                kind_hint: None,
                origin: "explicit".into(),
            },
        )
        .await
        .unwrap();
    let original = owner
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Initial note",
        ))
        .await
        .unwrap();
    let request = ReviseMemoryInput {
        producer: None,
        subject,
        operation_id: OperationId::new(),
        memory_id: original.object.memory_id,
        expected_object_epoch: original.object.object_epoch,
        intent: nous_memory::RevisionIntent::Correct,
        formation_mode: nous_memory::FormationMode::Grounded,
        grounding_occurrence_id: Some(source.occurrence.occurrence_id),
        semantic_role: "fact".into(),
        representation_text: "Revised note".into(),
        title: None,
        basis: original.basis.clone(),
        aboutness: vec![],
        tags: vec![tag.tag_id, tag.tag_id],
        valid_time: TemporalExtent::Unknown,
        epistemic_class: nous_core::EpistemicClass::Observed,
    };
    let revised = owner.revise_memory(request.clone()).await.unwrap();
    let replay = owner.revise_memory(request).await.unwrap();
    assert_eq!(revised.tags, vec![tag.tag_id]);
    assert_eq!(
        replay.revision.memory_revision_id,
        revised.revision.memory_revision_id
    );
    let service = nous_kernel::transport::KernelService(runtime.clone());
    let request = |token| {
        tonic::Request::new(p::MemoryHistoryRequest {
            subject_id: subject.0.to_string(),
            memory_id: original.object.memory_id.0.to_string(),
            page: Some(p::Page {
                page_size: 1,
                page_token: token,
            }),
        })
    };
    let first = service
        .list_memory_revisions(request(String::new()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.items.len(), 1);
    assert_eq!(
        first.items[0].revision_id,
        original.revision.memory_revision_id.0.to_string()
    );
    assert!(!first.items[0].basis.is_empty());
    assert!(!first.items[0].acceptance_state.is_empty());
    assert!(first.items[0].created_at.is_some());
    assert!(!first.next_page_token.is_empty());
    let second = service
        .list_memory_revisions(request(first.next_page_token))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(second.items.len(), 1);
    assert_eq!(
        second.items[0].revision_id,
        revised.revision.memory_revision_id.0.to_string()
    );
    assert_eq!(second.items[0].tags, vec![tag.tag_id.0.to_string()]);
    assert!(second.next_page_token.is_empty());
    owner
        .form_memory(form_input(
            subject,
            source.occurrence.occurrence_id,
            OperationId::new(),
            "Another note",
        ))
        .await
        .unwrap();
    let list = |token| {
        tonic::Request::new(p::ListRequest {
            subject_id: subject.0.to_string(),
            page: Some(p::Page {
                page_size: 1,
                page_token: token,
            }),
            ..Default::default()
        })
    };
    let page_one = service
        .list_memories(list(String::new()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page_one.items.len(), 1);
    assert!(!page_one.next_page_token.is_empty());
    let page_two = service
        .list_memories(list(page_one.next_page_token))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page_two.items.len(), 1);
    assert_ne!(page_one.items[0].memory_id, page_two.items[0].memory_id);
    assert!(page_two.next_page_token.is_empty());
}
