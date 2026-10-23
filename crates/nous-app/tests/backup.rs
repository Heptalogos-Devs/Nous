use anyhow::{Context, Result};
use nous_app::{
    App,
    backup::restore,
    common::instance::{InstanceConfig, InstanceGuard, InstancePaths},
};
use nous_domain::CallContext;
use nous_domain::{Capability, Material, Namespace, Observation, Subject};
use std::fs;
use std::path::PathBuf;

#[tokio::test]
async fn snapshot_restore_preserves_authority_and_objects_with_new_credentials() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../temp/tests");
    fs::create_dir_all(&root)?;
    let scratch = tempfile::tempdir_in(root)?;
    let source = InstancePaths::new(scratch.path().join("source"))?;
    source.ensure_layout()?;
    let _guard = InstanceGuard::acquire(&source)?;
    assert!(InstanceGuard::acquire(&source).is_err());
    assert!(InstanceGuard::is_running(&source)?);
    source.initialize()?;
    let prompt_text = "当前实例绑定的提示词，快照恢复后继续使用。";
    fs::write(source.prompts.join("memory.md"), prompt_text)?;
    let mut config = InstanceConfig::load(&source)?;
    config.models.memory_formation = Some(serde_json::from_value(serde_json::json!({
        "resource":{"id":"backup-formation","provider":"local-test",
            "protocol":"openai_chat_completions","endpoint":"http://127.0.0.1:1/v1/",
            "model":"unused","credential":{"file":"credentials/formation.token"}},
        "prompt":{"file":"prompts/memory.md"}
    }))?);
    fs::write(&source.config, toml::to_string_pretty(&config)?)?;
    let source_token = source.read_secret(&InstanceConfig::load(&source)?.auth.admin_token)?;
    fs::write(
        source.credentials.join("not-exported.key"),
        b"private-provider-key",
    )?;
    let app = App::open(source.clone()).await?;
    // Back up the active bootstrap snapshot even when the file is edited later.
    fs::write(
        &source.config,
        toml::to_string_pretty(&InstanceConfig::default())?,
    )?;
    let admin = CallContext::admin();
    let namespace: Namespace = serde_json::from_value(
        app.create_namespace(&admin, "backup-ns", "notes")
            .await?
            .result,
    )?;
    let subject: Subject = serde_json::from_value(
        app.create_subject(
            &admin,
            "backup-subject",
            &namespace.id,
            "reader",
            vec![Capability::Memory],
        )
        .await?
        .result,
    )?;
    let material: Material = serde_json::from_value(
        app.put_material(
            &admin,
            "backup-text",
            &namespace.id,
            "快照恢复后仍然能读取的材料",
            Some("backup-test".into()),
            None,
        )
        .await?
        .result,
    )?;
    let observation: Observation = serde_json::from_value(
        app.record_observation(
            &admin,
            "backup-observation",
            &subject.id,
            &material.id,
            None,
        )
        .await?
        .result,
    )?;
    let created = app.create_backup(&admin).await?;
    let backup = PathBuf::from(created["backup"].as_str().context("missing backup path")?);
    assert!(!backup.join("credentials").exists());
    assert_eq!(
        fs::read_to_string(backup.join("prompts/memory.md"))?,
        prompt_text
    );
    let target = InstancePaths::new(scratch.path().join("restored"))?;
    let result = restore(&backup, target.clone()).await?;
    assert_eq!(result["state"], "restored");
    assert_ne!(
        target.read_secret(&InstanceConfig::load(&target)?.auth.admin_token)?,
        source_token
    );
    assert!(!target.credentials.join("not-exported.key").exists());
    assert_eq!(
        fs::read_to_string(target.prompts.join("memory.md"))?,
        prompt_text
    );
    assert_eq!(
        InstanceConfig::load(&target)?
            .models
            .memory_formation
            .context("missing restored formation binding")?
            .prompt
            .file,
        Some(PathBuf::from("prompts/memory.md"))
    );
    let restored = App::open(target.clone()).await?;
    assert_eq!(
        restored.get_material(&admin, &material.id).await?.text,
        "快照恢复后仍然能读取的材料"
    );
    assert_eq!(
        restored.list_observations(&admin, &subject.id).await?[0].id,
        observation.id
    );
    assert_eq!(restored.list_namespaces(&admin).await?[0].id, namespace.id);
    restored.close().await;
    assert!(restore(&backup, target.clone()).await.is_err());
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(backup.join("manifest.json"))?)?;
    let object = manifest["files"]
        .as_array()
        .context("missing manifest files")?
        .iter()
        .find(|file| file["sha256"] == material.content_hash)
        .context("missing material object")?;
    fs::write(
        backup.join(object["path"].as_str().context("missing object path")?),
        b"corrupt",
    )?;
    let rejected = InstancePaths::new(scratch.path().join("rejected"))?;
    assert!(restore(&backup, rejected.clone()).await.is_err());
    assert!(!rejected.root.exists());
    app.close().await;
    Ok(())
}
