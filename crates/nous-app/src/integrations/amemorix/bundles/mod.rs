mod input;
mod mapping;
mod presentation;
pub use input::BundleInput;

use super::AmemorixRequest;
use crate::{
    App,
    bundles::{InspectBundleRequest, ResolveBundleFileRequest},
};
use nous_domain::*;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &BundleInput,
) -> Result<Value> {
    let cancel = CancellationToken::new();
    let _cancel_on_drop = cancel.clone().drop_guard();
    match input {
        BundleInput::Export(input) => export(app, ctx, binding, input, &cancel).await,
        BundleInput::Inspect { material_id } => {
            let summary = app
                .inspect_bundle(
                    ctx,
                    InspectBundleRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        material_id: material_id.clone(),
                    },
                    &cancel,
                )
                .await?;
            Ok(presentation::inspection(summary))
        }
        BundleInput::Import(input) => {
            let placement = mapping::placement(input)?;
            let receipt = app
                .install_bundle(
                    ctx,
                    &input.operation_id,
                    InstallBundleRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        material_id: input.material_id.clone(),
                        mode: input.mode,
                        placement,
                    },
                    &cancel,
                )
                .await?;
            presentation::installed(receipt, input)
        }
        BundleInput::List { limit } => {
            let mut installations = app
                .bundle_installations(ctx, &binding.namespace_id, &binding.subject_id)
                .await?;
            installations.sort_by(|a, b| {
                b.uninstalled_at
                    .as_ref()
                    .unwrap_or(&b.installed_at)
                    .cmp(a.uninstalled_at.as_ref().unwrap_or(&a.installed_at))
                    .then(a.id.cmp(&b.id))
            });
            let items = installations
                .into_iter()
                .take((*limit).clamp(1, 200))
                .map(presentation::installation)
                .collect::<Result<Vec<_>>>()?;
            Ok(json!({"success":true,"count":items.len(),"items":items}))
        }
        BundleInput::ResolveFile { file_name } => {
            let mut value = app
                .resolve_bundle_file(
                    ctx,
                    ResolveBundleFileRequest {
                        namespace_id: binding.namespace_id.clone(),
                        subject_id: binding.subject_id.clone(),
                        file_name: file_name.clone(),
                    },
                    &cancel,
                )
                .await?;
            value["success"] = json!(true);
            Ok(value)
        }
        BundleInput::Uninstall {
            installation_id,
            operation_id,
        } => {
            let value = app.bundle_installation(ctx, installation_id).await?;
            let installation: BundleInstallation =
                serde_json::from_value(value["installation"].clone())?;
            if installation.namespace_id != binding.namespace_id
                || installation.subject_id != binding.subject_id
            {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "installation is not in the requested subject",
                ));
            }
            presentation::uninstalled(
                app.uninstall_bundle(ctx, operation_id, installation_id)
                    .await?,
            )
        }
    }
}

async fn export(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &input::ExportInput,
    cancel: &CancellationToken,
) -> Result<Value> {
    let mut request = mapping::export_request(&binding.namespace_id, &binding.subject_id, input)?;
    if input.preview || mapping::automatic_id(input) {
        let mut summary = app.preview_bundle(ctx, request.clone(), cancel).await?;
        if mapping::automatic_id(input) {
            let digest = summary
                .manifest
                .get("content_digest")
                .and_then(Value::as_str)
                .filter(|digest| digest.len() >= 16 && digest.is_ascii())
                .ok_or_else(|| {
                    Error::internal("portable preview has no semantic content digest")
                })?;
            request.package.id = format!("amx.{}", &digest[..16]);
            summary.package.id = request.package.id.clone();
            summary.manifest["package"]["id"] = json!(request.package.id);
        }
        if input.preview {
            return Ok(
                json!({"success":true,"preview":true,"manifest":summary.manifest,
                "counts":summary.counts,"uncompressed_size":summary.uncompressed_size,
                "package":summary.package,"content_level":summary.content_level,
                "source_watermark":summary.source_watermark}),
            );
        }
    }
    let receipt = app
        .export_bundle(ctx, &input.operation_id, request, cancel)
        .await?;
    let value = &receipt.result;
    Ok(
        json!({"success":true,"file_name":value["file_name"],"size":value["size"],
        "download_material_id":value["download_material_id"],
        "manifest":value["export"]["manifest"],"counts":value["export"]["counts"],
        "package":value["export"]["package"],"content_level":value["export"]["content_level"],
        "uncompressed_size":value["export"]["uncompressed_size"],
        "operation_id":receipt.operation_id,"native":receipt}),
    )
}
