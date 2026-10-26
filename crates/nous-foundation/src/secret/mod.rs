//! Protected secret files and recovery of access credentials awaiting authority ownership.
mod files;
mod publication;
mod value;
use crate::platform::permissions::PrivatePermissions;
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
    permissions: PrivatePermissions,
    active: Mutex<BTreeSet<SecretRef>>,
}

impl Secrets {
    pub fn new(paths: Arc<InstancePaths>) -> Result<Self> {
        let permissions = PrivatePermissions::for_instance(&paths.root)?;
        Ok(Self::owned(paths, permissions))
    }
    pub fn for_staging(instance: &InstancePaths, paths: Arc<InstancePaths>) -> Result<Self> {
        let root = std::fs::canonicalize(&paths.root)?;
        let staging = std::fs::canonicalize(
            instance
                .resolve("temp/staging")
                .map_err(files::path_error)?,
        )?;
        if !crate::instance::is_within(&root, &staging) {
            return Err(Error::invalid(
                "secret staging must belong to its final instance",
            ));
        }
        Ok(Self::owned(
            paths,
            PrivatePermissions::for_instance(&instance.root)?,
        ))
    }
    fn owned(paths: Arc<InstancePaths>, permissions: PrivatePermissions) -> Self {
        Self {
            inner: Arc::new(Inner {
                paths,
                permissions,
                active: Mutex::new(BTreeSet::new()),
            }),
        }
    }
    pub async fn prepare_runtime(&self) -> Result<()> {
        let paths = self.inner.paths.clone();
        let permissions = self.inner.permissions.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::prepare(&paths, &permissions)
        })
        .await
    }
    pub async fn prepare_instance_access(&self) -> Result<()> {
        let owner = self.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            owner.prepare_instance_access_blocking()
        })
        .await
    }
    fn prepare_instance_access_blocking(&self) -> Result<()> {
        self.inner.permissions.protect_root()?;
        self.inner
            .permissions
            .protect_directory(&self.inner.paths.root)?;
        files::prepare(&self.inner.paths, &self.inner.permissions)
    }
    pub async fn read(&self, reference: &SecretRef) -> Result<SecretValue> {
        let paths = self.inner.paths.clone();
        let permissions = self.inner.permissions.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::read(&paths, &permissions, &reference)
        })
        .await
    }
    pub async fn ensure_bootstrap(&self, reference: &SecretRef) -> Result<SecretValue> {
        let paths = self.inner.paths.clone();
        let permissions = self.inner.permissions.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::ensure_bootstrap(&paths, &permissions, &reference)
        })
        .await
    }
    pub(crate) fn ensure_bootstrap_blocking(&self, reference: &SecretRef) -> Result<SecretValue> {
        self.prepare_instance_access_blocking()?;
        files::ensure_bootstrap(&self.inner.paths, &self.inner.permissions, reference)
    }
    pub(crate) fn read_bootstrap_blocking(&self, reference: &SecretRef) -> Result<SecretValue> {
        files::prepare(&self.inner.paths, &self.inner.permissions)?;
        files::read(&self.inner.paths, &self.inner.permissions, reference)
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
        let permissions = self.inner.permissions.clone();
        let reference = reference.clone();
        execution::blocking(&CancellationToken::new(), move |_| {
            files::write_new(&paths, &permissions, &reference, &value)
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
        self.prepare_runtime().await?;
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
