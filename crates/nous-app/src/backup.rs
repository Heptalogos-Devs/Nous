//! Complete instance snapshots: authority data, referenced objects and configuration.
use crate::App;
use anyhow::{Context, Result, ensure};
use nous_domain::{CallContext, Error, ErrorCode};
use nous_foundation::{
    clock::Clock,
    content, execution,
    identity::{AuthConfig, Identity},
    instance::{InstanceConfig, InstanceGuard, InstancePaths},
    prompts::{ManagedPromptReference, Prompts},
    secret::Secrets,
};
use nous_storage::Database;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
const DATABASE_FILE: &str = "data/authority.sqlite";
const CONFIG_FILE: &str = "instance.toml";
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
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
        let app = self.clone();
        execution::owned_async(&CancellationToken::new(), move |cancel| async move {
            app.create_backup_inner(cancel).await.map_err(backup_error)
        })
        .await
    }
    async fn create_backup_inner(&self, cancel: CancellationToken) -> Result<Value> {
        let captured = Clock::operation_snapshot();
        let _protection = self.objects.protect_references(&cancel).await?;
        let staging_root = self.paths.resolve("temp/staging")?;
        let staging = execution::blocking(&cancel, move |_| {
            tempfile::Builder::new()
                .prefix("backup-")
                .tempdir_in(staging_root)
                .map_err(Error::from)
        })
        .await?;
        let database_path = staging.path().join(DATABASE_FILE);
        tokio::fs::create_dir_all(database_path.parent().context("database has no parent")?)
            .await?;
        self.database.backup_to(&database_path).await?;
        let snapshot = Database::open(&database_path, Clock::operation_snapshot).await?;
        let metadata: Result<_> = async {
            let entries = snapshot.configuration_entries_all().await?;
            validate_managed_prompts(&snapshot, self.config.as_ref(), &entries).await?;
            Ok((
                snapshot.object_references().await?,
                snapshot.status().await?.commit_seq,
                entries,
            ))
        }
        .await;
        snapshot.close().await;
        let (references, commit_seq, configuration_entries) = metadata?;
        let mut config = self.config.as_ref().clone();
        config.auth = AuthConfig::default();
        let prompt_files = prompt_references(self.paths.as_ref(), &config, &configuration_entries)?;
        let config_text = toml::to_string_pretty(&config)?;
        let root = staging.path().to_owned();
        let mut files = Vec::new();
        let mut seen = HashMap::new();
        for reference in references {
            let path = object_name(&reference.content_hash)?;
            if let Some(length) = seen.insert(path.clone(), reference.byte_len) {
                ensure!(
                    length == reference.byte_len,
                    "inconsistent lengths for a referenced object"
                );
                continue;
            }
            self.objects
                .copy_to(
                    &reference.content_hash,
                    reference.byte_len,
                    &root.join(&path),
                    &cancel,
                )
                .await?;
            files.push(BackupFile {
                path,
                sha256: reference.content_hash,
                byte_len: reference.byte_len as u64,
            });
        }
        let mut copied = HashMap::new();
        for reference in prompt_files {
            let name = reference
                .relative_path
                .to_str()
                .context("prompt path is not UTF-8")?
                .replace('\\', "/");
            if let Some(previous) = copied.insert(name.clone(), reference.sha256.clone()) {
                ensure!(
                    previous == reference.sha256,
                    "prompt changed while capturing resource references"
                );
                continue;
            }
            content::copy_verified(
                self.paths.clone(),
                &reference.path,
                &root.join(&name),
                &reference.sha256,
                reference.byte_len,
                &cancel,
            )
            .await?;
            files.push(BackupFile {
                path: name,
                sha256: reference.sha256,
                byte_len: reference.byte_len,
            });
        }
        let config_root = root.clone();
        execution::blocking(&cancel, move |_| {
            write_new(&config_root.join(CONFIG_FILE), config_text.as_bytes()).map_err(backup_error)
        })
        .await?;
        files.extend([
            describe(&root, CONFIG_FILE, &cancel).await?,
            describe(&root, DATABASE_FILE, &cancel).await?,
        ]);
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = Manifest {
            created_at: captured.recorded(),
            commit_seq,
            files,
        };
        let manifest_root = root.clone();
        let manifest = execution::blocking(&cancel, move |_| {
            write_new(
                &manifest_root.join("manifest.json"),
                &serde_json::to_vec_pretty(&manifest)?,
            )
            .map_err(backup_error)?;
            sync_directory(&manifest_root).map_err(backup_error)?;
            Ok(manifest)
        })
        .await?;
        let name = format!(
            "{}-{}",
            captured.utc().format("%Y%m%dT%H%M%SZ"),
            uuid::Uuid::new_v4()
        );
        let destination = self.paths.resolve(format!("backups/{name}"))?;
        let target = destination.clone();
        let paths = self.paths.clone();
        execution::blocking(&cancel, move |_| {
            let result: Result<()> = (|| {
                ensure!(!target.exists(), "backup destination already exists");
                move_owned(staging.path(), &target, &paths)?;
                sync_directory(&paths.backups)
            })();
            result.map_err(backup_error)
        })
        .await?;
        Ok(
            json!({"backup":destination,"commit_seq":manifest.commit_seq,"files":manifest.files.len(),"credentials":"excluded","indexes":"rebuild_on_start"}),
        )
    }
}

pub async fn restore(source: &Path, target: InstancePaths) -> Result<Value> {
    let source = source.to_owned();
    execution::owned_async(&CancellationToken::new(), move |cancel| async move {
        restore_inner(&source, target, cancel)
            .await
            .map_err(backup_error)
    })
    .await
    .map_err(Into::into)
}
async fn restore_inner(
    source: &Path,
    target: InstancePaths,
    cancel: CancellationToken,
) -> Result<Value> {
    let source = tokio::fs::canonicalize(source)
        .await
        .context("backup directory cannot be resolved")?;
    let manifest_path = source.join("manifest.json");
    ensure!(
        tokio::fs::canonicalize(&manifest_path)
            .await?
            .starts_with(&source),
        "manifest escapes the backup directory"
    );
    let manifest: Manifest = serde_json::from_slice(&tokio::fs::read(manifest_path).await?)
        .context("invalid backup manifest")?;
    validate_manifest(&manifest)?;
    for file in &manifest.files {
        verify_source(&source, file, &cancel).await?;
    }
    let (target, _guard, staging, staged) = execution::blocking(&cancel, move |_| {
        let setup: Result<_> = (|| {
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
            let guard = InstanceGuard::acquire(&target)?;
            let staging = tempfile::Builder::new()
                .prefix("restore-")
                .tempdir_in(target.resolve("temp/staging")?)?;
            let staged = InstancePaths::new(staging.path())?;
            staged.ensure_layout()?;
            nous_foundation::instance::initialize(&staged)?;
            Ok((target, guard, staging, staged))
        })();
        setup.map_err(backup_error)
    })
    .await?;
    let copy_paths = Arc::new(target.clone());
    let _protection = content::ObjectStore::new(copy_paths.clone())
        .protect_references(&cancel)
        .await?;
    for file in &manifest.files {
        if file.path == CONFIG_FILE {
            let config_source = source.join(CONFIG_FILE);
            let config_target = staged.config.clone();
            let file = file.clone();
            execution::blocking(&cancel, move |_| {
                let update: Result<()> = (|| {
                    let bytes = fs::read(config_source)?;
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
                        .open(config_target)?;
                    output.write_all(text.as_bytes())?;
                    output.sync_all()?;
                    Ok(())
                })();
                update.map_err(backup_error)
            })
            .await?;
        } else {
            content::copy_verified(
                copy_paths.clone(),
                &source.join(&file.path),
                &staged.root.join(&file.path),
                &file.sha256,
                file.byte_len,
                &cancel,
            )
            .await?;
        }
    }
    let snapshot = Database::open(&staged.database, Clock::operation_snapshot).await?;
    let checked: Result<()> = async {
        ensure!(
            snapshot.status().await?.commit_seq == manifest.commit_seq,
            "backup commit sequence does not match its authority"
        );
        let listed = manifest
            .files
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect::<HashMap<_, _>>();
        let mut expected = HashSet::from([CONFIG_FILE.to_owned(), DATABASE_FILE.to_owned()]);
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
        let config = InstanceConfig::load(&staged)?;
        let entries = snapshot.configuration_entries_all().await?;
        validate_managed_prompts(&snapshot, &config, &entries).await?;
        for reference in prompt_references(&staged, &config, &entries)? {
            let name = reference
                .relative_path
                .to_str()
                .context("prompt path is not UTF-8")?
                .replace('\\', "/");
            let file = listed
                .get(name.as_str())
                .context("backup omits a bound prompt resource")?;
            ensure!(
                file.sha256 == reference.sha256 && file.byte_len == reference.byte_len,
                "backup prompt metadata is inconsistent"
            );
            expected.insert(name);
        }
        ensure!(
            expected == listed.keys().map(|key| (*key).to_owned()).collect(),
            "backup includes files outside the authority and resource snapshot"
        );
        Ok(())
    }
    .await;
    let finalization: Result<i64> = async {
        checked?;
        let auth=AuthConfig::default();
        let secrets=Secrets::new(Arc::new(staged.clone()));
        let value=secrets.read(&auth.admin_token).await?;
        let identity=Identity::new(snapshot.clone(),secrets,Clock);
        let ctx=CallContext::new("restore",nous_domain::AccessScope::Admin);
        let key=nous_storage::operation_key(&ctx,"identity.restore","instance",&uuid::Uuid::new_v4().to_string(),&json!({"seed":auth.admin_token}))?;
        let mut operation=match snapshot.begin_operation(key).await?{
            nous_storage::BeginOperation::New(operation)=>operation,
            nous_storage::BeginOperation::Replay(_)=>unreachable!("fresh restore identity operation"),
        };
        let grant=identity.reset_restored_in(&mut operation,&auth.admin_token,Secrets::digest(&value)).await?;
        let seed=nous_foundation::configuration::install_bootstrap_seed_in(&mut operation,&auth).await?;
        operation.commit(json!({"grant":grant.reference(),"seed_revision":seed.revision,"issued_credentials_revoked":true})).await?;
        Ok(snapshot.status().await?.commit_seq)
    }
    .await;
    snapshot.close().await;
    let restored_commit_seq = finalization?;
    let root = target.root.clone();
    let credential = target.credentials.join("admin.token");
    execution::blocking(&cancel, move |_| {
        let directories = ["data", "objects", "credentials", "prompts"];
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
            sync_directory(&target.root)
        })();
        if let Err(error) = publish {
            if config_published {
                move_owned(&target.config, &staged.config, &target).map_err(backup_error)?;
            }
            for directory in published.into_iter().rev() {
                let _ = move_owned(
                    &target.resolve(directory).map_err(backup_error)?,
                    &staged.root.join(directory),
                    &target,
                );
            }
            return Err(backup_error(error.context("restore publication failed")));
        }
        drop(staging);
        Ok(())
    })
    .await?;
    Ok(
        json!({"state":"restored","instance":root,"source_commit_seq":manifest.commit_seq,"commit_seq":restored_commit_seq,"administrator_token_file":credential,"credentials":"new_administrator","indexes":"rebuild_on_start"}),
    )
}
fn validate_manifest(manifest: &Manifest) -> Result<()> {
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
        if file.path != CONFIG_FILE
            && file.path != DATABASE_FILE
            && !file.path.starts_with("prompts/")
        {
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
async fn describe(root: &Path, name: &str, cancel: &CancellationToken) -> Result<BackupFile> {
    let (sha256, byte_len) = content::digest_file(&root.join(name), cancel).await?;
    Ok(BackupFile {
        path: name.to_owned(),
        sha256,
        byte_len,
    })
}
async fn verify_source(root: &Path, file: &BackupFile, cancel: &CancellationToken) -> Result<()> {
    let path = tokio::fs::canonicalize(root.join(&file.path)).await?;
    ensure!(
        path.starts_with(root),
        "backup file escapes the source directory"
    );
    content::verify_file(&path, &file.sha256, file.byte_len, cancel).await?;
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
    fs::File::open(path)?.sync_all()?;
    Ok(())
}
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<()> {
    Ok(())
}

fn prompt_references(
    paths: &InstancePaths,
    config: &InstanceConfig,
    entries: &[nous_domain::ConfigEntry],
) -> Result<Vec<ManagedPromptReference>> {
    let roles = crate::composition::model_roles()?;
    roles
        .prompt_bindings(&config.models, entries)?
        .into_iter()
        .filter_map(|(_, binding)| Prompts::file_reference(paths, &binding).transpose())
        .collect::<nous_domain::Result<_>>()
        .map_err(Into::into)
}

async fn validate_managed_prompts(
    snapshot: &Database,
    config: &InstanceConfig,
    entries: &[nous_domain::ConfigEntry],
) -> Result<()> {
    let bindings = crate::composition::model_roles()?
        .prompt_bindings(&config.models, entries)?
        .into_iter()
        .map(|(_, binding)| binding)
        .collect::<Vec<_>>();
    Prompts::validate_managed_bindings(snapshot, &bindings).await?;
    Ok(())
}
fn backup_error(error: anyhow::Error) -> Error {
    match error.downcast_ref::<Error>() {
        Some(error) => error.clone(),
        None => Error::new(ErrorCode::DependencyUnavailable, "backup operation failed")
            .with_cause(error.as_ref()),
    }
}
