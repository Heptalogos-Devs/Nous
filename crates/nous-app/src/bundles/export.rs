use super::*;
use serde::Serialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct BundleExportSummary {
    pub format: BundleExportFormat,
    pub package: PackageDescriptor,
    pub content_level: BundleContentLevel,
    pub manifest: Value,
    pub counts: BTreeMap<String, u64>,
    pub uncompressed_size: u64,
    pub source_watermark: i64,
}
struct ExportArtifact {
    staging: tempfile::TempDir,
    path: PathBuf,
    summary: BundleExportSummary,
}

impl App {
    pub async fn preview_bundle(
        &self,
        ctx: &CallContext,
        request: ExportBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<BundleExportSummary> {
        let app = self.clone();
        let context = ctx.clone();
        execution::owned_async(cancel, move |local| async move {
            let _protection = app.objects.protect_references(&local).await?;
            let (package, staging, _) = app
                .prepare_bundle_export(&context, &request, &local)
                .await?;
            let root = staging.path().to_path_buf();
            execution::blocking(&local, move |_| {
                let summary = match request.format {
                    BundleExportFormat::Amemorix => {
                        let projected = portable::preview(
                            &root,
                            &package,
                            &request.assets.unwrap_or_default(),
                        )?;
                        BundleExportSummary {
                            format: request.format,
                            package: package.package,
                            content_level: package.content_level,
                            manifest: projected.manifest,
                            counts: projected.counts,
                            uncompressed_size: projected.uncompressed_size,
                            source_watermark: package.watermark,
                        }
                    }
                    BundleExportFormat::Native => native_summary(&package)?,
                };
                drop(staging);
                Ok(summary)
            })
            .await
        })
        .await
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

    async fn prepare_bundle_export(
        &self,
        ctx: &CallContext,
        request: &ExportBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<(SubjectPackage, tempfile::TempDir, policy::BundlePolicy)> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        request.validate()?;
        let policy = self
            .bundle_policy(&request.namespace_id, &request.subject_id)
            .await?;
        let mut view = self.database.read_view().await?;
        let package = view
            .subject_package(ctx, request, Clock::operation_snapshot().recorded())
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
        Ok((package, staging, policy))
    }

    async fn export_bundle_inner(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ExportBundleRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        request.validate()?;
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
        let protection = self.objects.protect_references(cancel).await?;
        let (package, staging, policy) = self.prepare_bundle_export(ctx, &request, cancel).await?;
        let root = staging.path().to_path_buf();
        let limits = policy.limits.clone();
        let format = request.format;
        let artifact = execution::blocking(cancel, move |local| {
            let (path, summary) = match format {
                BundleExportFormat::Native => {
                    let path = archive::write(&root, &package, &local)?;
                    archive::load(&path, &limits)?.ok_or_else(|| {
                        Error::internal("exported subject archive could not be read")
                    })?;
                    (path, native_summary(&package)?)
                }
                BundleExportFormat::Amemorix => {
                    let artifact = portable::write(
                        &root,
                        &package,
                        &request.assets.unwrap_or_default(),
                        &local,
                    )?;
                    crate::bundle_format::inspect(std::fs::File::open(&artifact.path)?, &limits)
                        .map_err(archive::format_error)?;
                    let summary = BundleExportSummary {
                        format,
                        package: package.package,
                        content_level: package.content_level,
                        manifest: artifact.summary.manifest,
                        counts: artifact.summary.counts,
                        uncompressed_size: artifact.summary.uncompressed_size,
                        source_watermark: package.watermark,
                    };
                    (artifact.path, summary)
                }
            };
            if std::fs::metadata(&path)?.len() > limits.archive_bytes {
                return Err(Error::new(
                    ErrorCode::ResourceExhausted,
                    "exported bundle exceeds configured archive limit",
                ));
            }
            Ok(ExportArtifact {
                staging,
                path,
                summary,
            })
        })
        .await?;
        let published = self
            .objects
            .publish(
                tokio::fs::File::open(&artifact.path).await?,
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
            media_type: match format {
                BundleExportFormat::Native => "application/vnd.nous.subject+zip",
                BundleExportFormat::Amemorix => "application/vnd.a-memorix.bundle+zip",
            }
            .into(),
            metadata: MediaMetadata::Binary,
            byte_len: published.byte_len,
            source: Some(format!("nous-bundle:{}", artifact.summary.package.id)),
            event_time: None,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        let extension = match format {
            BundleExportFormat::Native => "nousbundle",
            BundleExportFormat::Amemorix => "amembundle",
        };
        let file_name = format!("bundle-{}.{}", material.id, extension);
        tx.insert_material(&material).await?;
        tx.retain_material_output(&material.namespace_id, &request.subject_id, &material.id)
            .await?;
        tx.set_material_details(&material.namespace_id, &material.id, &MaterialDetails {
            source_type: Some("nous_bundle".into()), tags: vec![],
            metadata: json!({"export":artifact.summary,"subject_id":request.subject_id,"file_name":file_name}),
            structured: None,
        }).await?;
        let receipt = tx
            .commit(json!({
                "material":material, "download_material_id":material.id, "file_name":file_name,
                "size":material.byte_len, "export":artifact.summary,
            }))
            .await?;
        drop(artifact.staging);
        Ok(receipt)
    }
}

fn native_summary(package: &SubjectPackage) -> Result<BundleExportSummary> {
    let document = serde_json::to_value(package)?;
    let manifest = json!({
        "format":archive::FORMAT,"package":package.package,"content_level":package.content_level,
        "content_digest":archive::content_digest(&document)?,"document_digest":archive::digest(&document)?,
    });
    let mut files = BTreeMap::from([
        ("knowledge.json".to_owned(), archive::digest(&document)?),
        ("manifest.json".to_owned(), archive::digest(&manifest)?),
    ]);
    let mut uncompressed_size =
        serde_json::to_vec(&document)?.len() as u64 + serde_json::to_vec(&manifest)?.len() as u64;
    for blob in package.objects()? {
        files.insert(format!("objects/{}", blob.content_hash), blob.content_hash);
        uncompressed_size = uncompressed_size
            .checked_add(blob.byte_len)
            .ok_or_else(|| Error::invalid("bundle uncompressed size overflow"))?;
    }
    uncompressed_size = uncompressed_size
        .checked_add(serde_json::to_vec(&json!({"algorithm":"sha256","files":files}))?.len() as u64)
        .ok_or_else(|| Error::invalid("bundle uncompressed size overflow"))?;
    Ok(BundleExportSummary {
        format: BundleExportFormat::Native,
        package: package.package.clone(),
        content_level: package.content_level,
        manifest,
        counts: inspection::package_counts(package),
        uncompressed_size,
        source_watermark: package.watermark,
    })
}
