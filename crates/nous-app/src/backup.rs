//! Complete instance snapshots: authoritative SQLite data, its referenced objects and configuration.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path},
};

use anyhow::{Context, Result, ensure};
use chrono::Utc;
use nous_domain::{CallContext, Error, ErrorCode};
use nous_storage::Database;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    App,
    instance::{AuthConfig, InstanceConfig, InstanceGuard, InstancePaths},
};

const FORMAT_VERSION: u32 = 1;
const DATABASE_FILE: &str = "data/authority.sqlite";
const CONFIG_FILE: &str = "instance.toml";

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    created_at: String,
    commit_seq: i64,
    files: Vec<BackupFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupFile {
    path: String,
    sha256: String,
    byte_len: u64,
}

impl App {
    pub async fn create_backup(&self, ctx: &CallContext) -> nous_domain::Result<Value> {
        ctx.require_admin()?;
        self.create_backup_inner().await.map_err(|error| {
            Error::new(
                ErrorCode::DependencyUnavailable,
                format!("backup failed: {error:#}"),
            )
        })
    }

    async fn create_backup_inner(&self) -> Result<Value> {
        let staging_root = self.paths.resolve("temp/staging")?;
        let staging = tempfile::Builder::new()
            .prefix("backup-")
            .tempdir_in(staging_root)?;
        let database_path = staging.path().join(DATABASE_FILE);
        fs::create_dir_all(database_path.parent().context("database has no parent")?)?;
        self.database.backup_to(&database_path).await?;
        let snapshot = Database::open(&database_path).await?;
        let metadata: Result<_> = async {
            Ok((
                snapshot.object_references().await?,
                snapshot.status().await?.commit_seq,
            ))
        }
        .await;
        snapshot.close().await;
        let (references, commit_seq) = metadata?;
        let mut config = InstanceConfig::load(&self.paths)?;
        config.auth = AuthConfig::default();
        let config_text = toml::to_string_pretty(&config)?;
        let root = staging.path().to_owned();
        let objects = self.objects.clone();
        let manifest = tokio::task::spawn_blocking(move || -> Result<Manifest> {
            write_new(&root.join(CONFIG_FILE), config_text.as_bytes())?;
            let mut files = vec![
                describe(&root, CONFIG_FILE)?,
                describe(&root, DATABASE_FILE)?,
            ];
            let mut seen = HashMap::new();
            for reference in references {
                let path = object_name(&reference.content_hash)?;
                if let Some(byte_len) = seen.insert(path.clone(), reference.byte_len) {
                    ensure!(
                        byte_len == reference.byte_len,
                        "inconsistent lengths for a referenced object"
                    );
                    continue;
                }
                let bytes = objects.read(&reference.content_hash, reference.byte_len)?;
                write_new(&root.join(&path), &bytes)?;
                files.push(BackupFile {
                    path,
                    sha256: reference.content_hash,
                    byte_len: bytes.len() as u64,
                });
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            let manifest = Manifest {
                format_version: FORMAT_VERSION,
                created_at: Utc::now().to_rfc3339(),
                commit_seq,
                files,
            };
            write_new(
                &root.join("manifest.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )?;
            Ok(manifest)
        })
        .await
        .context("backup copy task failed")??;
        let name = format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            uuid::Uuid::new_v4()
        );
        let destination = self.paths.resolve(format!("backups/{name}"))?;
        ensure!(!destination.exists(), "backup destination already exists");
        move_owned(staging.path(), &destination, &self.paths)?;
        sync_directory(&self.paths.backups)?;
        Ok(
            json!({"backup":destination,"format_version":FORMAT_VERSION,"commit_seq":manifest.commit_seq,"files":manifest.files.len(),"credentials":"excluded","indexes":"rebuild_on_start"}),
        )
    }
}

/// Restore into a new, empty target instance. Credentials are created locally for the new host.
pub async fn restore(source: &Path, target: InstancePaths) -> Result<Value> {
    let source = fs::canonicalize(source).context("backup directory cannot be resolved")?;
    let manifest_path = source.join("manifest.json");
    ensure!(
        fs::canonicalize(&manifest_path)?.starts_with(&source),
        "manifest escapes the backup directory"
    );
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(&manifest_path)?).context("invalid backup manifest")?;
    validate_manifest(&manifest)?;
    // Validate the supplied snapshot before creating the target instance's layout.
    for file in &manifest.files {
        verify_source(&source, file)?;
    }
    ensure!(
        !target.config.exists() && !target.database.exists(),
        "refusing to overwrite an existing instance"
    );
    if target.root.exists() {
        ensure!(
            fs::read_dir(&target.root)?.next().is_none(),
            "restore target must be an empty directory"
        );
    }
    target.ensure_layout()?;
    let _guard = InstanceGuard::acquire(&target)?;
    let staging = tempfile::Builder::new()
        .prefix("restore-")
        .tempdir_in(target.resolve("temp/staging")?)?;
    let staged = InstancePaths::new(staging.path())?;
    staged.ensure_layout()?;
    // Initialize before copying the backed configuration so the new admin secret is independent.
    staged.initialize()?;
    let copy_root = staged.root.clone();
    let copy_source = source.clone();
    let copy_files = manifest.files.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        for file in copy_files {
            if file.path == CONFIG_FILE {
                let bytes = fs::read(copy_source.join(CONFIG_FILE))?;
                ensure!(
                    bytes.len() as u64 == file.byte_len
                        && format!("{:x}", Sha256::digest(&bytes)) == file.sha256,
                    "backup configuration changed while restoring"
                );
                let mut config: InstanceConfig = toml::from_str(std::str::from_utf8(&bytes)?)?;
                config.auth = AuthConfig::default();
                config.validate()?;
                let text = toml::to_string_pretty(&config)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(copy_root.join(CONFIG_FILE))?;
                output.write_all(text.as_bytes())?;
                output.sync_all()?;
            } else {
                copy_verified(
                    &copy_source.join(&file.path),
                    &copy_root.join(&file.path),
                    &file,
                )?;
            }
        }
        Ok(())
    })
    .await
    .context("restore copy task failed")??;
    let snapshot = Database::open(&staged.database).await?;
    let checked: Result<()> = async {
        ensure!(
            snapshot.status().await?.commit_seq == manifest.commit_seq,
            "backup commit sequence does not match its database"
        );
        let listed: HashMap<&str, &BackupFile> = manifest
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        let mut expected: HashSet<String> = [CONFIG_FILE.to_owned(), DATABASE_FILE.to_owned()]
            .into_iter()
            .collect();
        for reference in snapshot.object_references().await? {
            let name = object_name(&reference.content_hash)?;
            let file = listed
                .get(name.as_str())
                .context("backup omits a referenced object")?;
            ensure!(
                reference.byte_len >= 0
                    && file.byte_len == reference.byte_len as u64
                    && file.sha256 == reference.content_hash,
                "backup object reference metadata is inconsistent"
            );
            expected.insert(name);
        }
        ensure!(
            expected.len() == listed.len(),
            "backup includes objects outside the authority snapshot"
        );
        Ok(())
    }
    .await;
    snapshot.close().await;
    checked?;
    // All moves stay inside target.root. Publish configuration last, after complete data and credentials.
    let directories = ["data", "objects", "credentials"];
    let mut published = Vec::new();
    let mut config_published = false;
    let publish: Result<()> = (|| {
        for directory in directories {
            let destination = target.resolve(directory)?;
            ensure!(
                fs::read_dir(&destination)?.next().is_none(),
                "restore destination became nonempty"
            );
            fs::remove_dir(&destination)?;
            move_owned(&staged.resolve(directory)?, &destination, &target)?;
            published.push(directory);
        }
        move_owned(&staged.config, &target.config, &target)?;
        config_published = true;
        sync_directory(&target.root)?;
        Ok(())
    })();
    if let Err(error) = publish {
        if config_published {
            move_owned(&target.config, &staged.config, &target)
                .context("restore publication failed and configuration rollback failed")?;
        }
        for directory in published.into_iter().rev() {
            let _ = move_owned(
                &target.resolve(directory)?,
                &staged.root.join(directory),
                &target,
            );
        }
        return Err(error.context("restore publication failed"));
    }
    Ok(
        json!({"state":"restored","instance":target.root,"commit_seq":manifest.commit_seq,"administrator_token_file":target.credentials.join("admin.token"),"credentials":"new_administrator","indexes":"rebuild_on_start"}),
    )
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.format_version == FORMAT_VERSION,
        "unsupported backup manifest version"
    );
    ensure!(manifest.commit_seq >= 0, "invalid backup commit sequence");
    let mut seen = HashSet::new();
    for file in &manifest.files {
        let path = Path::new(&file.path);
        ensure!(
            !file.path.contains('\\')
                && path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
            "backup file paths must be relative normal paths"
        );
        ensure!(is_hash(&file.sha256), "invalid backup file digest");
        ensure!(
            seen.insert(file.path.as_str()),
            "duplicate backup file path"
        );
        if file.path != CONFIG_FILE && file.path != DATABASE_FILE {
            let parts: Vec<_> = file.path.split('/').collect();
            ensure!(
                parts.len() == 3
                    && parts[0] == "objects"
                    && is_hash(parts[2])
                    && parts[1] == &parts[2][..2]
                    && file.sha256 == parts[2],
                "backup contains an unsupported file path"
            );
        }
    }
    ensure!(
        seen.contains(CONFIG_FILE) && seen.contains(DATABASE_FILE),
        "backup requires configuration and authority database"
    );
    Ok(())
}
fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn object_name(hash: &str) -> Result<String> {
    ensure!(is_hash(hash), "invalid object digest in authority snapshot");
    Ok(format!("objects/{}/{hash}", &hash[..2]))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("file has no parent")?)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn describe(root: &Path, name: &str) -> Result<BackupFile> {
    let (sha256, byte_len) = digest(&root.join(name))?;
    Ok(BackupFile {
        path: name.to_owned(),
        sha256,
        byte_len,
    })
}
fn digest(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut byte_len = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        byte_len += count as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), byte_len))
}
fn verify_source(root: &Path, file: &BackupFile) -> Result<()> {
    let path = fs::canonicalize(root.join(&file.path))?;
    ensure!(
        path.starts_with(root),
        "backup file escapes the source directory"
    );
    let (sha256, byte_len) = digest(&path)?;
    ensure!(
        sha256 == file.sha256 && byte_len == file.byte_len,
        "backup integrity check failed for {}",
        file.path
    );
    Ok(())
}
fn copy_verified(source: &Path, target: &Path, file: &BackupFile) -> Result<()> {
    fs::create_dir_all(target.parent().context("file has no parent")?)?;
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let mut hasher = Sha256::new();
    let mut byte_len = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        byte_len += count as u64;
    }
    ensure!(
        format!("{:x}", hasher.finalize()) == file.sha256 && byte_len == file.byte_len,
        "backup changed while copying {}",
        file.path
    );
    output.sync_all()?;
    Ok(())
}
fn move_owned(source: &Path, target: &Path, instance: &InstancePaths) -> Result<()> {
    let source = instance.resolve(
        source
            .strip_prefix(&instance.root)
            .context("move source is outside the target instance")?,
    )?;
    let target = instance.resolve(
        target
            .strip_prefix(&instance.root)
            .context("move destination is outside the target instance")?,
    )?;
    fs::rename(source, target)?;
    Ok(())
}
#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nous_domain::{Capability, Material, Namespace, Observation, Subject};
    use std::path::PathBuf;

    #[tokio::test]
    async fn snapshot_restore_preserves_authority_and_objects_with_new_credentials() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let source = InstancePaths::new(scratch.path().join("source"))?;
        source.ensure_layout()?;
        let _guard = InstanceGuard::acquire(&source)?;
        source.initialize()?;
        let source_token = source.read_secret(&InstanceConfig::load(&source)?.auth.admin_token)?;
        fs::write(
            source.credentials.join("not-exported.key"),
            b"private-provider-key",
        )?;
        let app = App::open(source.clone()).await?;
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
        let target = InstancePaths::new(scratch.path().join("restored"))?;
        let result = restore(&backup, target.clone()).await?;
        assert_eq!(result["state"], "restored");
        assert_ne!(
            target.read_secret(&InstanceConfig::load(&target)?.auth.admin_token)?,
            source_token
        );
        assert!(!target.credentials.join("not-exported.key").exists());
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
        fs::write(
            backup.join(object_name(&material.content_hash)?),
            b"corrupt",
        )?;
        let rejected = InstancePaths::new(scratch.path().join("rejected"))?;
        assert!(restore(&backup, rejected.clone()).await.is_err());
        assert!(!rejected.root.exists());
        app.close().await;
        Ok(())
    }
}
