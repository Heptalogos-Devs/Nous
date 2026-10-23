use nous_app::{App, common::instance::InstancePaths};
use nous_domain::*;

async fn instance() -> (tempfile::TempDir, InstancePaths, App) {
    let parent =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../temp/instances/tests");
    std::fs::create_dir_all(&parent).unwrap();
    let directory = tempfile::tempdir_in(parent).unwrap();
    let paths = InstancePaths::new(directory.path()).unwrap();
    paths.ensure_layout().unwrap();
    let app = App::open(paths.clone()).await.unwrap();
    (directory, paths, app)
}

#[tokio::test]
async fn operations_replay_across_restart_and_reject_changed_payload() {
    let (_directory, paths, app) = instance().await;
    let ctx = CallContext::admin();
    let result = app
        .create_namespace(&ctx, "first", "research")
        .await
        .unwrap();
    let replay = app
        .create_namespace(&ctx, "first", "research")
        .await
        .unwrap();
    assert_eq!(result, replay);
    assert_eq!(
        app.create_namespace(&ctx, "first", "changed")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    // Racing callers share the same authority result and sequence.
    let (a, b) = tokio::join!(
        app.create_namespace(&ctx, "parallel", "shared"),
        app.create_namespace(&ctx, "parallel", "shared")
    );
    assert_eq!(a.unwrap(), b.unwrap());
    app.close().await;
    let reopened = App::open(paths).await.unwrap();
    assert_eq!(
        result,
        reopened
            .create_namespace(&ctx, "first", "research")
            .await
            .unwrap()
    );
    assert_eq!(reopened.list_namespaces(&ctx).await.unwrap().len(), 2);
    reopened.close().await;
}

#[tokio::test]
async fn materials_and_actual_contacts_have_distinct_identity_and_access() {
    let (_directory, paths, app) = instance().await;
    let admin = CallContext::admin();
    let ns: Namespace = serde_json::from_value(
        app.create_namespace(&admin, "ns", "scope")
            .await
            .unwrap()
            .result,
    )
    .unwrap();
    let other: Namespace = serde_json::from_value(
        app.create_namespace(&admin, "ns-other", "other")
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
    let scoped = CallContext {
        caller_id: "subject-reader".into(),
        scope: AccessScope::Subject {
            namespace_id: ns.id.clone(),
            subject_id: subject.id.clone(),
        },
    };
    let outside = CallContext {
        caller_id: "outside".into(),
        scope: AccessScope::Namespace {
            namespace_id: other.id,
        },
    };
    let material: Material = serde_json::from_value(
        app.put_material(
            &admin,
            "text",
            &ns.id,
            "A real source. 红叶在秋天变色。",
            Some("test:source".into()),
            None,
        )
        .await
        .unwrap()
        .result,
    )
    .unwrap();
    assert!(
        app.list_observations(&admin, &subject.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.get_material(&outside, &material.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        app.get_material(&scoped, &material.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        app.record_observation(&scoped, "self-grant", &subject.id, &material.id, None)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let first = app
        .record_observation(&admin, "contact1", &subject.id, &material.id, None)
        .await
        .unwrap();
    let second = app
        .record_observation(
            &scoped,
            "contact2",
            &subject.id,
            &material.id,
            Some(TimePoint {
                domain: "world:study".into(),
                coordinate: 12,
            }),
        )
        .await
        .unwrap();
    assert_ne!(first.result["id"], second.result["id"]);
    let observations = app.list_observations(&scoped, &subject.id).await.unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations[0].observed_at.is_none());
    assert_eq!(
        app.get_material(&scoped, &material.id).await.unwrap().text,
        "A real source. 红叶在秋天变色。"
    );
    app.close().await;
    let reopened = App::open(paths.clone()).await.unwrap();
    assert_eq!(
        reopened
            .list_observations(&admin, &subject.id)
            .await
            .unwrap()
            .len(),
        2
    );
    // Source reads verify immutable bytes rather than silently returning corruption.
    let object = paths
        .objects
        .join(&material.content_hash[..2])
        .join(&material.content_hash);
    std::fs::write(object, "corrupted").unwrap();
    assert_eq!(
        reopened
            .get_material(&admin, &material.id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::DependencyUnavailable
    );
    reopened.close().await;
}
