//! Portable subject knowledge, validated archive IO and installation ownership.
mod archive;
mod export;
mod foreign;
mod portable;
mod units;
pub use export::BundleExportSummary;
mod files;
mod input;
mod inspection;
pub use files::ResolveBundleFileRequest;
pub use inspection::{BundleInspectionSummary, InspectBundleRequest};
pub mod policy;
use crate::App;
use nous_domain::{media::*, *};
use nous_foundation::{clock::Clock, execution};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

impl App {
    async fn bundle_policy(
        &self,
        ns: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<policy::BundlePolicy> {
        self.configuration
            .snapshot(
                &ConfigScope::Subject {
                    namespace_id: ns.clone(),
                    subject_id: subject.clone(),
                },
                false,
            )
            .await?
            .get(policy::BUNDLES)
    }
    pub async fn install_bundle(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: InstallBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        let app = self.clone();
        let context = ctx.clone();
        let operation_id = operation_id.to_owned();
        execution::owned_async(cancel, move |local| async move {
            app.install_bundle_inner(&context, &operation_id, request, &local)
                .await
        })
        .await
    }
    async fn install_bundle_inner(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: InstallBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(
            ctx,
            "bundle.install",
            &scope,
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let input = self
            .bundle_input(
                ctx,
                &request.namespace_id,
                &request.subject_id,
                &request.material_id,
                cancel,
            )
            .await?;
        let protection = self.objects.protect_references(cancel).await?;
        let input::BundleInput {
            original: _original,
            staging,
            path,
            policy,
        } = input;
        let limits = policy.limits.clone();
        let root = staging.path().to_path_buf();
        let (staging, loaded) = execution::blocking(cancel, move |local| {
            let loaded = match archive::load(&path, &limits)? {
                Some(loaded) => loaded,
                None => {
                    let inspection =
                        crate::bundle_format::inspect(std::fs::File::open(&path)?, &limits)
                            .map_err(archive::format_error)?;
                    foreign::convert(inspection, &root)?
                }
            };
            archive::extract(
                &path,
                &root,
                &loaded.members,
                &loaded.object_members,
                &local,
            )?;
            Ok((staging, loaded))
        })
        .await?;
        let mut objects = Vec::new();
        for blob in loaded.package.objects()? {
            let object = self
                .objects
                .publish(
                    tokio::fs::File::open(staging.path().join(&blob.content_hash)).await?,
                    policy.limits.member_bytes,
                    cancel,
                )
                .await?;
            if object.digest != blob.content_hash || object.byte_len as u64 != blob.byte_len {
                return Err(Error::invalid(
                    "staged bundle object differs from its authority reference",
                ));
            }
            objects.push(object);
        }
        self.verify_bundle_unit_sources(&loaded.package, cancel)
            .await?;
        execution::check_cancelled(cancel)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        tx.retain_until_end(protection);
        for object in &objects {
            object.protect_in(&mut tx);
        }
        let installation = tx
            .install_subject_package(&loaded.package, &request, &loaded.digest)
            .await?;
        let vectors = tx.bundle_vector_dispositions(&installation.id).await?;
        let receipt=tx.commit(json!({"installation":installation,"archive_material_id":request.material_id,"vector_contents":loaded.package.vectors.contents.len(),"vectors":vectors,"unaccepted_external_vector_assets":loaded.unaccepted_vector_members})).await?;
        drop(staging);
        Ok(receipt)
    }
    pub async fn bundle_installations(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
    ) -> Result<Vec<BundleInstallation>> {
        ctx.require_subject(namespace, subject)?;
        self.database.bundle_installations(namespace, subject).await
    }
    pub async fn bundle_installation(&self, ctx: &CallContext, id: &str) -> Result<Value> {
        let installation = self.database.bundle_installation(id).await?;
        ctx.require_subject(&installation.namespace_id, &installation.subject_id)?;
        Ok(
            json!({"installation":installation,"resources":self.database.bundle_resources(id).await?,"imports":self.database.bundle_ancestry(id).await?,"vectors":self.database.bundle_vector_dispositions(id).await?}),
        )
    }
    pub async fn uninstall_bundle(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        id: &str,
    ) -> Result<OperationReceipt> {
        let installation = self.database.bundle_installation(id).await?;
        ctx.require_subject(&installation.namespace_id, &installation.subject_id)?;
        let scope = format!(
            "namespace:{}/subject:{}",
            installation.namespace_id, installation.subject_id
        );
        let key = operation_key(
            ctx,
            "bundle.uninstall",
            &scope,
            operation_id,
            &json!({"installation_id":id}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let withdrawal = tx.withdraw_bundle(id).await?;
        let receipt = tx.commit(serde_json::to_value(withdrawal)?).await?;
        Ok(receipt)
    }
    async fn bundle_staging(&self, cancel: &CancellationToken) -> Result<tempfile::TempDir> {
        let root = self.paths.resolve("temp/staging").map_err(|e| {
            Error::new(
                ErrorCode::DependencyUnavailable,
                "cannot resolve bundle staging directory",
            )
            .with_cause(e.as_ref())
        })?;
        execution::blocking(cancel, move |_| {
            tempfile::Builder::new()
                .prefix("bundle-")
                .tempdir_in(root)
                .map_err(Error::from)
        })
        .await
    }
}
