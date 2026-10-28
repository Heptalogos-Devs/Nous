use super::*;
use nous_app::bundles::{InspectBundleRequest, ResolveBundleFileRequest};
use nous_domain::{ExportBundleRequest, InstallBundleRequest, NamespaceId, SubjectId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BundleOperation {
    Export {
        operation_id: String,
        request: ExportBundleRequest,
    },
    Preview {
        request: ExportBundleRequest,
    },
    Inspect {
        request: InspectBundleRequest,
    },
    ResolveFile {
        request: ResolveBundleFileRequest,
    },
    Install {
        operation_id: String,
        request: InstallBundleRequest,
    },
    List {
        namespace_id: NamespaceId,
        subject_id: SubjectId,
    },
    Get {
        installation_id: String,
    },
    Uninstall {
        operation_id: String,
        installation_id: String,
    },
}
impl BundleOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        let cancel = CancellationToken::new();
        let _cancel_on_drop = cancel.clone().drop_guard();
        Ok(match self {
            Self::Preview { request } => {
                json!(app.preview_bundle(ctx, request.clone(), &cancel).await?)
            }
            Self::Inspect { request } => {
                json!(app.inspect_bundle(ctx, request.clone(), &cancel).await?)
            }
            Self::ResolveFile { request } => {
                app.resolve_bundle_file(ctx, request.clone(), &cancel)
                    .await?
            }
            Self::Export {
                operation_id,
                request,
            } => json!(
                app.export_bundle(ctx, operation_id, request.clone(), &cancel)
                    .await?
            ),
            Self::Install {
                operation_id,
                request,
            } => json!(
                app.install_bundle(ctx, operation_id, request.clone(), &cancel)
                    .await?
            ),
            Self::List {
                namespace_id,
                subject_id,
            } => json!(
                app.bundle_installations(ctx, namespace_id, subject_id)
                    .await?
            ),
            Self::Get { installation_id } => app.bundle_installation(ctx, installation_id).await?,
            Self::Uninstall {
                operation_id,
                installation_id,
            } => json!(
                app.uninstall_bundle(ctx, operation_id, installation_id)
                    .await?
            ),
        })
    }
}
pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/bundles", post(bundles))
}
async fn bundles(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<BundleOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Bundle(Box::new(decode(body)?))).await
}
