use super::*;
use nous_domain::{MaterialId, SubjectId, media::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaOperation {
    Inspect {
        material_id: MaterialId,
    },
    Source {
        subject_id: SubjectId,
        reference: MediaReference,
    },
    Lineage {
        subject_id: SubjectId,
        reference: MediaReference,
    },
    Derive {
        operation_id: String,
        request: DeriveMediaRequest,
    },
    Run {
        job_id: String,
    },
    Representation {
        subject_id: SubjectId,
        reference: RepresentationRef,
    },
    History {
        subject_id: SubjectId,
        representation_id: DerivedRepresentationId,
        limit: usize,
    },
    CreateSourceRegion {
        operation_id: String,
        request: SourceRegionRequest,
    },
    SourceRegion {
        subject_id: SubjectId,
        id: SourceRegionId,
    },
    CreateDerivedRegion {
        operation_id: String,
        request: DerivedRegionRequest,
    },
    DerivedRegion {
        subject_id: SubjectId,
        id: DerivedRegionId,
    },
}
impl MediaOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        Ok(match self {
            Self::Inspect { material_id } => json!(app.inspect_material(ctx, material_id).await?),
            Self::Source {
                subject_id,
                reference,
            } => json!(app.read_media_source(ctx, subject_id, reference).await?),
            Self::Lineage {
                subject_id,
                reference,
            } => json!(app.media_source_lineage(ctx, subject_id, reference).await?),
            Self::Derive {
                operation_id,
                request,
            } => json!(app.derive_media(ctx, operation_id, request.clone()).await?),
            Self::Run { job_id } => json!(app.media_derivation(ctx, job_id).await?),
            Self::Representation {
                subject_id,
                reference,
            } => json!(app.media_representation(ctx, subject_id, reference).await?),
            Self::History {
                subject_id,
                representation_id,
                limit,
            } => json!(
                app.media_representation_history(ctx, subject_id, representation_id, *limit)
                    .await?
            ),
            Self::CreateSourceRegion {
                operation_id,
                request,
            } => json!(
                app.create_source_region(ctx, operation_id, request.clone())
                    .await?
            ),
            Self::SourceRegion { subject_id, id } => {
                json!(app.source_region(ctx, subject_id, id).await?)
            }
            Self::CreateDerivedRegion {
                operation_id,
                request,
            } => json!(
                app.create_derived_region(ctx, operation_id, request.clone())
                    .await?
            ),
            Self::DerivedRegion { subject_id, id } => {
                json!(app.derived_region(ctx, subject_id, id).await?)
            }
        })
    }
}
pub(super) fn router() -> Router<ApiState> {
    Router::new().route("/api/media", post(media))
}
async fn media(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    input: Result<Json<MediaOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Media(Box::new(decode(input)?))).await
}
