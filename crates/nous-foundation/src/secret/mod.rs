//! Protected secret files and recovery of access credentials awaiting authority ownership.
mod files;
mod publication;
mod value;
use crate::{execution, instance::InstancePaths};
pub use nous_domain::SecretRef;
use nous_domain::{Error, GrantId, Result};
use nous_storage::Database;
pub use publication::PreparedCredential;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
pub use value::SecretValue;

#[derive(Clone, Debug)]
pub struct Secrets {
    inner: Arc<Inner>,
}
#[derive(Debug)]
struct Inner {
    paths: Arc<InstancePaths>,
    active: Mutex<BTreeSet<SecretRef>>,
}

impl Secrets {
    pub fn new(paths: Arc<InstancePaths>) -> Self {
        Self {
            inner: Arc::new(Inner {
                paths,
                active: Mutex::new(BTreeSet::new()),
            }),
        }
    }
    pub async fn read(&self, reference: &SecretRef) -> Result<SecretValue> {
        let paths = self.inner.paths.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::read(&paths, &reference)
        })
        .await
    }
    pub async fn ensure_bootstrap(&self, reference: &SecretRef) -> Result<SecretValue> {
        let paths = self.inner.paths.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::ensure_bootstrap(&paths, &reference)
        })
        .await
    }
    pub fn ensure_bootstrap_blocking(
        paths: &InstancePaths,
        reference: &SecretRef,
    ) -> Result<SecretValue> {
        files::ensure_bootstrap(paths, reference)
    }
    pub fn read_bootstrap_blocking(
        paths: &InstancePaths,
        reference: &SecretRef,
    ) -> Result<SecretValue> {
        files::read(paths, reference)
    }
    pub async fn read_token_file(source: &Path) -> Result<SecretValue> {
        let source = source.to_owned();
        execution::blocking(&CancellationToken::new(), move |_| {
            use std::io::Read;
            let mut value = String::new();
            std::fs::File::open(source)?
                .take(4097)
                .read_to_string(&mut value)?;
            SecretValue::parse(value)
        })
        .await
    }
    pub async fn write_new(&self, reference: &SecretRef, value: SecretValue) -> Result<()> {
        if reference.file.starts_with("credentials/access") {
            return Err(Error::invalid(
                "generated access credentials are published by the identity owner",
            ));
        }
        let paths = self.inner.paths.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::write_new(&paths, &reference, &value)
        })
        .await
    }
    pub async fn prepare_access(
        &self,
        database: Database,
        grant: &GrantId,
    ) -> Result<PreparedCredential> {
        grant.validate()?;
        let value = SecretValue::parse(format!("{}.{}", grant, Self::random_value().expose()))?;
        publication::prepare(self.clone(), database, value).await
    }
    pub async fn reconcile(&self, database: &Database) -> Result<u64> {
        publication::reconcile(self, database).await
    }
    pub fn digest(value: &SecretValue) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(value.expose().as_bytes()))
    }
    fn random_value() -> SecretValue {
        SecretValue(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ))
    }
    fn active(&self) -> Result<std::sync::MutexGuard<'_, BTreeSet<SecretRef>>> {
        self.inner
            .active
            .lock()
            .map_err(|_| Error::internal("secret publication lock is poisoned"))
    }
}
