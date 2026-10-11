// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

mod test_support;
use nous_core::{OperationId, TemporalExtent};
use nous_memory::{CreateTagRequest, ReviseMemoryInput};
use nous_protocol::public as p;
use p::memory_service_server::MemoryService as MemoryRpcService;
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
        memory_id: original.header.memory_id(),
        expected_object_epoch: original.header.object_epoch,
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
        replay.revision.header.revision_id(),
        revised.revision.header.revision_id()
    );
    let service = nous_kernel::transport::KernelService(runtime.clone());
    let request = |token| {
        tonic::Request::new(p::CognitionHistoryRequest {
            subject_id: subject.0.to_string(),
            object_ref: Some(p::CognitiveRef {
                kind: "memory".into(),
                value: original.header.memory_id().0.to_string(),
            }),
            page: Some(p::Page {
                page_size: 1,
                page_token: token,
            }),
        })
    };
    let first = service
        .list_cognition_history(request(String::new()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(first.items.len(), 1);
    let p::cognition_result::Result::Memory(first_memory) = first.items[0].result.as_ref().unwrap()
    else {
        panic!("history item should be Memory");
    };
    assert_eq!(
        first_memory
            .revision
            .as_ref()
            .unwrap()
            .header
            .as_ref()
            .unwrap()
            .revision_ref
            .as_ref()
            .unwrap()
            .value,
        original.revision.header.revision_id().to_string()
    );
    assert!(
        !first_memory
            .revision
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .basis
            .is_empty()
    );
    assert!(
        !first_memory
            .header
            .as_ref()
            .unwrap()
            .acceptance_state
            .is_empty()
    );
    assert!(first_memory.header.as_ref().unwrap().created_at.is_some());
    assert!(!first.next_page_token.is_empty());
    let second = service
        .list_cognition_history(request(first.next_page_token))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(second.items.len(), 1);
    let p::cognition_result::Result::Memory(second_memory) =
        second.items[0].result.as_ref().unwrap()
    else {
        panic!("history item should be Memory");
    };
    assert_eq!(
        second_memory
            .revision
            .as_ref()
            .unwrap()
            .header
            .as_ref()
            .unwrap()
            .revision_ref
            .as_ref()
            .unwrap()
            .value,
        revised.revision.header.revision_id().to_string()
    );
    assert_eq!(
        second_memory
            .revision
            .as_ref()
            .unwrap()
            .details
            .as_ref()
            .unwrap()
            .tags,
        vec![tag.tag_id.0.to_string()]
    );
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
        tonic::Request::new(p::CognitionListRequest {
            subject_id: subject.0.to_string(),
            kind: "memory".into(),
            page: Some(p::Page {
                page_size: 1,
                page_token: token,
            }),
            status: "accepted".into(),
        })
    };
    let page_one = service
        .list_cognitions(list(String::new()))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page_one.items.len(), 1);
    assert!(!page_one.next_page_token.is_empty());
    let page_two = service
        .list_cognitions(list(page_one.next_page_token))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page_two.items.len(), 1);
    let p::cognition_result::Result::Memory(page_one_memory) =
        page_one.items[0].result.as_ref().unwrap()
    else {
        panic!("list item should be Memory");
    };
    let p::cognition_result::Result::Memory(page_two_memory) =
        page_two.items[0].result.as_ref().unwrap()
    else {
        panic!("list item should be Memory");
    };
    assert_ne!(
        page_one_memory
            .header
            .as_ref()
            .unwrap()
            .object_ref
            .as_ref()
            .unwrap()
            .value,
        page_two_memory
            .header
            .as_ref()
            .unwrap()
            .object_ref
            .as_ref()
            .unwrap()
            .value
    );
    assert!(page_two.next_page_token.is_empty());
}
