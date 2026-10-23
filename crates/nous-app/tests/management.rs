use nous_app::{App, instance::InstancePaths};
use nous_domain::*;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn request(ns: NamespaceId, subject: SubjectId, content: String) -> ImportRequest {
    ImportRequest {
        namespace_id: ns,
        subject_id: subject,
        format: ImportFormat::Jsonl,
        content,
        form_memory: false,
        source: Some("test:m2-import".into()),
    }
}
fn query(subject: SubjectId, text: &str, inactive: bool) -> QueryRequest {
    QueryRequest {
        subject_id: subject,
        text: text.into(),
        reference: None,
        source: None,
        time: None,
        limit: 20,
        min_commit_seq: None,
        include_inactive: inactive,
    }
}
fn change(action: MemoryAction, revision: i64) -> MemoryChange {
    MemoryChange {
        action,
        expected_revision: revision,
        strength: None,
        hours: None,
        reason: "management regression".into(),
        force: false,
        edit: None,
    }
}
async fn mutate(
    app: &App,
    ctx: &CallContext,
    id: &MemoryId,
    action: MemoryAction,
    key: &str,
) -> Memory {
    let revision = app.get_memory(ctx, id).await.unwrap().revision;
    serde_json::from_value(
        app.change_memory(ctx, key, id, change(action, revision))
            .await
            .unwrap()
            .result,
    )
    .unwrap()
}

#[tokio::test]
async fn imported_memory_is_immediately_searchable_and_lifecycle_purge_survives_restart() {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../temp/instances/management-tests");
    std::fs::create_dir_all(&parent).unwrap();
    let scratch = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(scratch.path()).unwrap();
    paths.initialize().unwrap();
    let app = App::open(paths.clone()).await.unwrap();
    let admin = CallContext::admin();
    let ns: Namespace = serde_json::from_value(
        app.create_namespace(&admin, "ns", "import")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let subject: Subject = serde_json::from_value(
        app.create_subject(
            &admin,
            "subject",
            &ns.id,
            "reader",
            vec![Capability::Memory],
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    let actor = CallContext {
        caller_id: "reader-token".into(),
        scope: AccessScope::Subject {
            namespace_id: ns.id.clone(),
            subject_id: subject.id.clone(),
        },
    };
    let first = json!({"external_id":"note-a","text":"oldmarkerNW 京都的原始资料","metadata":{"z":2,"a":1},"structured":{"schema_version":1,"entities":["京都"],"relations":[["京都","位于","日本"]]}});
    let second = json!({"external_id":"note-b","text":"独立的第二份材料","tags":["样例"]});
    let content = format!("{first}\n{{broken}}\n{second}");
    let imported = app
        .import_text(
            &actor,
            request(ns.id.clone(), subject.id.clone(), content.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        (imported.accepted, imported.replayed, imported.rejected),
        (2, 0, 1),
        "{imported:#?}"
    );
    let receipt = &imported.items[0].receipt.as_ref().unwrap()["result"];
    let id: MemoryId = serde_json::from_value(receipt["memory_id"].clone()).unwrap();
    let material_id: MaterialId = serde_json::from_value(receipt["material_id"].clone()).unwrap();
    let source = app.memory_source(&actor, &id).await.unwrap();
    assert_eq!(source.memory.origin, MemoryOrigin::Imported);
    assert!(source.model_call.is_none());
    assert_eq!(source.observation.kind, ObservationKind::Import);
    assert!(
        app.list_observations(&actor, &subject.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        source.material.details.structured.as_ref().unwrap()["relations"][0][1],
        "位于"
    );
    assert_eq!(
        app.query(&actor, query(subject.id.clone(), "oldmarkerNW", false))
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
    let reversed = json!({"metadata":{"a":1,"z":2},"structured":first["structured"],"text":first["text"],"external_id":"note-a"});
    let replay = app
        .import_text(
            &actor,
            request(
                ns.id.clone(),
                subject.id.clone(),
                format!("{reversed}\n{{broken}}\n{second}"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        (replay.accepted, replay.replayed, replay.rejected),
        (0, 2, 1)
    );
    let conflict = app
        .import_text(
            &actor,
            request(
                ns.id.clone(),
                subject.id.clone(),
                json!({"external_id":"note-a","text":"different payload"}).to_string(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        conflict.items[0].error.as_ref().unwrap().code,
        ErrorCode::Conflict
    );
    let cancellation = CancellationToken::new();
    let worker = app.start_background(cancellation.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = app
                .query(&actor, query(subject.id.clone(), "oldmarkerNW", false))
                .await
                .unwrap();
            if result.coverage.indexed_through >= source.memory.commit_seq {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    cancellation.cancel();
    worker.await.unwrap();
    mutate(&app, &actor, &id, MemoryAction::Freeze, "freeze").await;
    assert!(
        app.query(&actor, query(subject.id.clone(), "oldmarkerNW", false))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    assert_eq!(
        app.query(&actor, query(subject.id.clone(), "oldmarkerNW", true))
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
    let protected = mutate(&app, &actor, &id, MemoryAction::Protect, "protect").await;
    assert!(protected.lifecycle.pinned);
    assert_eq!(
        app.delete_memory(&actor, "protected-delete", &id, protected.revision)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let reinforced = mutate(&app, &actor, &id, MemoryAction::Reinforce, "reinforce").await;
    assert_eq!(reinforced.lifecycle.activation, MemoryActivation::Active);
    let forgotten = mutate(&app, &actor, &id, MemoryAction::Forget, "forget").await;
    assert!(!forgotten.lifecycle.pinned);
    assert_eq!(forgotten.lifecycle.strength, 0.0);
    mutate(&app, &actor, &id, MemoryAction::Restore, "restore").await;
    let current = app.get_memory(&actor, &id).await.unwrap();
    let mut edit = change(MemoryAction::Revise, current.revision);
    edit.edit = Some(MemoryEdit {
        title: Some("修订记录".into()),
        content: Some("newmarkerNW 京都的修订内容".into()),
        epistemic_status: Some("uncertain".into()),
    });
    let revised: Memory = serde_json::from_value(
        app.change_memory(&actor, "revise", &id, edit)
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    assert!(
        app.query(&actor, query(subject.id.clone(), "oldmarkerNW", true))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    app.delete_memory(&actor, "trash", &id, revised.revision)
        .await
        .unwrap();
    let trash = app.recycle_bin(&actor, &subject.id, 100).await.unwrap();
    assert_eq!(trash.len(), 1);
    let mut purge = change(MemoryAction::Purge, trash[0].revision);
    purge.force = true;
    assert_eq!(
        app.change_memory(&actor, "purge", &id, purge.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let purged = app
        .change_memory(&admin, "purge", &id, purge.clone())
        .await
        .unwrap();
    assert!(purged.result["purged"].as_bool().unwrap());
    assert_eq!(
        app.change_memory(&admin, "purge", &id, purge)
            .await
            .unwrap(),
        purged
    );
    assert!(
        app.query(
            &actor,
            query(subject.id.clone(), "oldmarkerNW newmarkerNW", true)
        )
        .await
        .unwrap()
        .hits
        .is_empty()
    );
    assert_eq!(
        app.memory_history(&actor, &id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert!(
        app.memory_audit(&actor, &id, 100)
            .await
            .unwrap()
            .iter()
            .any(|event| event.action == "purge")
    );
    let old_receipt = app
        .delete_memory(&actor, "trash", &id, revised.revision)
        .await
        .unwrap();
    assert_eq!(old_receipt.result, json!({"id":id,"purged":true}));
    let replay = app
        .import_text(&actor, request(ns.id, subject.id.clone(), content))
        .await
        .unwrap();
    assert_eq!(replay.replayed, 2);
    assert_eq!(
        app.get_memory(&actor, &id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        app.get_material(&actor, &material_id).await.unwrap().text,
        "oldmarkerNW 京都的原始资料"
    );
    app.close().await;
    let reopened = App::open(paths).await.unwrap();
    assert!(
        reopened
            .query(&actor, query(subject.id, "oldmarkerNW newmarkerNW", true))
            .await
            .unwrap()
            .hits
            .is_empty()
    );
    assert!(
        reopened
            .memory_audit(&actor, &id, 100)
            .await
            .unwrap()
            .iter()
            .any(|event| event.action == "purge")
    );
    reopened.close().await;
}
