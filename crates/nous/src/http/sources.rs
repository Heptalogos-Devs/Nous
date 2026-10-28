use super::*;
use futures::{FutureExt, future::BoxFuture};
pub use nous_app::cognition::read::RetrievalSourceRequest;
use nous_domain::{
    RestoreSourcesRequest, SourceDirectoryRequest, SourceOperationRequest, SourceOperationsRequest,
    TrashSourcesRequest,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceManagementOperation {
    Preview {
        request: TrashSourcesRequest,
    },
    List {
        request: SourceDirectoryRequest,
    },
    Trash {
        operation_id: String,
        request: TrashSourcesRequest,
    },
    Restore {
        operation_id: String,
        request: RestoreSourcesRequest,
    },
    Get {
        request: SourceOperationRequest,
    },
    ListOperations {
        request: SourceOperationsRequest,
    },
    GetTrashOperation {
        request: SourceOperationRequest,
    },
    ListTrashOperations {
        request: SourceOperationsRequest,
    },
}
impl SourceManagementOperation {
    pub fn execute<'a>(
        &'a self,
        app: &'a App,
        ctx: &'a CallContext,
    ) -> BoxFuture<'a, Result<Value, Error>> {
        match self {
            Self::Preview { request } => async move {
                Ok(json!(app.preview_sources(ctx, request.clone()).await?))
            }
            .boxed(),
            Self::GetTrashOperation { request } => {
                async move { Ok(json!(app.source_trash_operation(ctx, request).await?)) }.boxed()
            }
            Self::ListTrashOperations { request } => {
                async move { Ok(json!(app.source_trash_operations(ctx, request).await?)) }.boxed()
            }
            Self::Get { request } => {
                async move { Ok(json!(app.source_operation(ctx, request).await?)) }.boxed()
            }
            Self::ListOperations { request } => {
                async move { Ok(json!(app.source_operations(ctx, request).await?)) }.boxed()
            }
            Self::List { request } => {
                async move { Ok(json!(app.source_directory(ctx, request).await?)) }.boxed()
            }
            Self::Trash {
                operation_id,
                request,
            } => async move {
                Ok(json!(
                    app.trash_sources(ctx, operation_id, request.clone())
                        .await?
                ))
            }
            .boxed(),
            Self::Restore {
                operation_id,
                request,
            } => async move {
                Ok(json!(
                    app.restore_sources(ctx, operation_id, request.clone())
                        .await?
                ))
            }
            .boxed(),
        }
    }
}

pub(super) fn router() -> Router<ApiState> {
    Router::new()
        .route("/api/sources/read", post(read))
        .route("/api/sources", post(manage))
}
async fn manage(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<SourceManagementOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(
        state,
        ctx,
        Operation::SourceManagement(Box::new(decode(body)?)),
    )
    .await
}
async fn read(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<RetrievalSourceRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::ReadSource(decode(body)?)).await
}
