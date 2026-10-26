//! Pending intent survives interruption until authority ownership can be read definitively.
use super::{SecretValue, Secrets, files};
use crate::{
    execution,
    observability::{current_trace, in_trace},
};
use nous_domain::{Error, Result, SecretRef, TraceId};
use nous_storage::{Database, WriteTransaction};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io::Write, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

#[derive(Debug)]
pub struct PreparedCredential {
    pub reference: SecretRef,
    pub digest: String,
    guard: Arc<CredentialGuard>,
}
impl PreparedCredential {
    pub fn protect_in(&self, operation: &mut WriteTransaction) {
        operation.retain_until_end(self.guard.clone());
    }
}

#[derive(Debug)]
struct CredentialGuard {
    owner: Secrets,
    database: Database,
    reference: SecretRef,
    trace: Option<TraceId>,
    span: tracing::Span,
}
impl Drop for CredentialGuard {
    fn drop(&mut self) {
        match self.owner.active() {
            Ok(mut active) => {
                active.remove(&self.reference);
            }
            Err(error) => {
                tracing::error!(event="secret.reconcile_deferred",error=?error);
                return;
            }
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let owner = self.owner.clone();
        let database = self.database.clone();
        let reference = self.reference.clone();
        let trace = self.trace.clone();
        let future = async move {
            let cleanup = async move {
                if let Err(error) = reconcile_one(&owner, &database, &reference).await {
                    tracing::warn!(event="secret.reconcile_deferred",error=?error);
                }
            };
            match trace {
                Some(trace) => in_trace(trace, cleanup).await,
                None => cleanup.await,
            }
        };
        let _cleanup = runtime.spawn(future.instrument(self.span.clone()));
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    reference: SecretRef,
}

pub(super) async fn prepare(
    owner: Secrets,
    database: Database,
    value: SecretValue,
) -> Result<PreparedCredential> {
    let id = uuid::Uuid::new_v4();
    let reference = SecretRef {
        file: PathBuf::from(format!("credentials/access/{id}.token")),
    };
    owner.active()?.insert(reference.clone());
    let guard = Arc::new(CredentialGuard {
        owner: owner.clone(),
        database,
        reference: reference.clone(),
        trace: current_trace(),
        span: tracing::Span::current(),
    });
    let digest = Secrets::digest(&value);
    let paths = owner.inner.paths.clone();
    let intent_reference = reference.clone();
    let publishing_guard = guard.clone();
    execution::blocking(&CancellationToken::new(), move |_| {
        let _guard = publishing_guard;
        let marker = marker(&paths, &intent_reference)?;
        let parent = marker
            .parent()
            .ok_or_else(|| Error::invalid("secret intent needs a parent"))?;
        std::fs::create_dir_all(parent)?;
        let mut staging = tempfile::NamedTempFile::new_in(parent)?;
        staging.write_all(&serde_json::to_vec(&Intent {
            reference: intent_reference.clone(),
        })?)?;
        staging.as_file().sync_all()?;
        staging
            .persist_noclobber(&marker)
            .map_err(|error| Error::from(error.error))?;
        files::write_new(&paths, &intent_reference, &value)
    })
    .await?;
    Ok(PreparedCredential {
        reference,
        digest,
        guard,
    })
}

pub(super) async fn reconcile(owner: &Secrets, database: &Database) -> Result<u64> {
    let paths = owner.inner.paths.clone();
    let references = execution::blocking(&CancellationToken::new(), move |_| {
        let mut references = BTreeSet::new();
        let directory = paths
            .resolve("temp/staging/secret-publications")
            .map_err(files::path_error)?;
        if directory.exists() {
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    let intent: Intent = serde_json::from_slice(&std::fs::read(entry.path())?)?;
                    access_reference(&intent.reference)?;
                    references.insert(intent.reference);
                }
            }
        }
        let directory = paths
            .resolve_in("credentials", "credentials/access")
            .map_err(files::path_error)?;
        if directory.exists() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                let reference = SecretRef {
                    file: PathBuf::from(format!("credentials/access/{name}")),
                };
                if entry.file_type()?.is_file() && access_reference(&reference).is_ok() {
                    references.insert(reference);
                }
            }
        }
        Ok(references)
    })
    .await?;
    let mut reclaimed = 0;
    for reference in references {
        reclaimed += u64::from(reconcile_one(owner, database, &reference).await?);
    }
    Ok(reclaimed)
}

async fn reconcile_one(
    owner: &Secrets,
    database: &Database,
    reference: &SecretRef,
) -> Result<bool> {
    access_reference(reference)?;
    if owner.active()?.contains(reference) {
        return Ok(false);
    }
    // A failed read leaves both intent and bytes in place; only a definite result authorizes cleanup.
    let referenced = database.grant_secret_referenced(reference).await?;
    let paths = owner.inner.paths.clone();
    let reference = reference.clone();
    let owner = owner.clone();
    execution::blocking(&CancellationToken::new(), move |_| {
        if owner.active()?.contains(&reference) {
            return Ok(false);
        }
        if !referenced {
            remove_if_present(files::path(&paths, &reference)?)?;
        }
        remove_if_present(marker(&paths, &reference)?)?;
        Ok(!referenced)
    })
    .await
}

fn access_reference(reference: &SecretRef) -> Result<()> {
    reference.validate()?;
    if reference.file.parent() != Some(std::path::Path::new("credentials/access"))
        || reference
            .file
            .extension()
            .is_none_or(|extension| extension != "token")
    {
        return Err(Error::invalid(
            "secret recovery intent is outside the generated access namespace",
        ));
    }
    let name = reference
        .file
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::invalid("invalid generated secret name"))?;
    let id = uuid::Uuid::parse_str(name)
        .map_err(|error| Error::invalid("invalid generated secret identity").with_cause(&error))?;
    if id.to_string() != name {
        return Err(Error::invalid("generated secret identity is not canonical"));
    }
    Ok(())
}
fn marker(paths: &crate::instance::InstancePaths, reference: &SecretRef) -> Result<PathBuf> {
    access_reference(reference)?;
    let name = reference
        .file
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::invalid("invalid generated secret name"))?;
    paths
        .resolve(format!("temp/staging/secret-publications/{name}.json"))
        .map_err(files::path_error)
}
fn remove_if_present(path: PathBuf) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
