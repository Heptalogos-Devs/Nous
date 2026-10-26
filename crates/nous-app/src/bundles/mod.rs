//! Portable subject knowledge, validated archive IO and installation ownership.
mod archive;
mod foreign;
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
    pub async fn export_bundle(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ExportBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        let app = self.clone();
        let context = ctx.clone();
        let operation_id = operation_id.to_owned();
        execution::owned_async(cancel, move |local| async move {
            app.export_bundle_inner(&context, &operation_id, request, &local)
                .await
        })
        .await
    }
    async fn export_bundle_inner(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ExportBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        validate_name(&request.package_id, "package ID")?;
        validate_name(&request.name, "package name")?;
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(
            ctx,
            "bundle.export",
            &scope,
            operation_id,
            &serde_json::to_value(&request)?,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let policy = self
            .bundle_policy(&request.namespace_id, &request.subject_id)
            .await?;
        let protection = self.objects.protect_references(cancel).await?;
        let mut view = self.database.read_view().await?;
        let package = view
            .subject_package(ctx, &request, Clock::operation_snapshot().recorded())
            .await?;
        view.finish().await?;
        let staging = self.bundle_staging(cancel).await?;
        for blob in package.objects()? {
            self.objects
                .copy_to(
                    &blob.content_hash,
                    blob.byte_len as i64,
                    &staging.path().join(&blob.content_hash),
                    cancel,
                )
                .await?;
        }
        let archive_package = package.clone();
        let root = staging.path().to_path_buf();
        let limits = policy.limits.clone();
        let (staging, path) = execution::blocking(cancel, move |local| {
            let path = archive::write(&root, &archive_package, &local)?;
            if std::fs::metadata(&path)?.len() > limits.archive_bytes {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "exported bundle exceeds configured archive limit",
                ));
            }
            archive::load(&path, &limits)?
                .ok_or_else(|| Error::internal("exported subject package could not be read"))?;
            Ok((staging, path))
        })
        .await?;
        let published = self
            .objects
            .publish(
                tokio::fs::File::open(path).await?,
                policy.limits.archive_bytes,
                cancel,
            )
            .await?;
        execution::check_cancelled(cancel)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        published.protect_in(&mut tx);
        tx.retain_until_end(protection);
        let material = Material {
            id: MaterialId::new(),
            namespace_id: request.namespace_id,
            content_hash: published.digest.clone(),
            media_type: "application/vnd.nous.subject+zip".into(),
            metadata: MediaMetadata::Binary,
            byte_len: published.byte_len,
            source: Some(format!("nous-bundle:{}", package.package_id)),
            event_time: None,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.insert_material(&material).await?;
        tx.set_material_details(&material.namespace_id,&material.id,&MaterialDetails{source_type:Some("nous_bundle".into()),tags:vec![],metadata:json!({"package_id":package.package_id,"subject_id":package.subject.id,"source_watermark":package.watermark}),structured:None}).await?;
        let receipt=tx.commit(json!({"material":material,"package_id":package.package_id,"content_digest":archive::digest(&package)?,"source_watermark":package.watermark,"download_material_id":material.id})).await?;
        drop(staging);
        Ok(receipt)
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
        let original = self
            .sources
            .original_metadata(ctx, &request.material_id, cancel)
            .await?;
        if original.material.namespace_id != request.namespace_id {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "bundle archive is outside the target namespace",
            ));
        }
        let policy = self
            .bundle_policy(&request.namespace_id, &request.subject_id)
            .await?;
        if original.material.byte_len as u64 > policy.limits.archive_bytes {
            return Err(Error::new(
                ErrorCode::ResourceExhausted,
                "bundle archive exceeds configured limit",
            ));
        }
        let protection = self.objects.protect_references(cancel).await?;
        let staging = self.bundle_staging(cancel).await?;
        let path = staging.path().join("uploaded.bundle");
        self.objects
            .copy_to(
                &original.material.content_hash,
                original.material.byte_len,
                &path,
                cancel,
            )
            .await?;
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
