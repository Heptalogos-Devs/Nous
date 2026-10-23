//! Typed protocol for structured cognition; all transports share this dispatch.
use crate::api::{ApiError, ApiState, Operation, decode, run};
use axum::{
    Json, Router,
    extract::{Extension, State, rejection::JsonRejection},
    routing::post,
};
use nous_app::{
    App,
    structured::{FormStructuredRequest, StructuredQuery},
};
use nous_domain::{
    CallContext, CognitiveId, CreateStructuredRequest, Error, MergeConceptsRequest,
    ResegmentEpisodesRequest, ReviseStructuredRequest, SplitConceptRequest, SubjectId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredOperation {
    Create {
        operation_id: String,
        input: CreateStructuredRequest,
    },
    Revise {
        operation_id: String,
        input: ReviseStructuredRequest,
    },
    Get {
        id: CognitiveId,
        revision: Option<i64>,
    },
    History {
        id: CognitiveId,
    },
    SetDeleted {
        operation_id: String,
        id: CognitiveId,
        expected_revision: i64,
        deleted: bool,
    },
    Source {
        id: CognitiveId,
        revision: Option<i64>,
    },
    ConceptResolve {
        subject_id: SubjectId,
        alias: String,
    },
    Query {
        input: StructuredQuery,
    },
    Form {
        operation_id: String,
        input: FormStructuredRequest,
    },
    FormationRun {
        job_id: String,
    },
    MergeConcepts {
        operation_id: String,
        input: MergeConceptsRequest,
    },
    SplitConcept {
        operation_id: String,
        input: SplitConceptRequest,
    },
    ResegmentEpisodes {
        operation_id: String,
        input: ResegmentEpisodesRequest,
    },
}
impl StructuredOperation {
    pub async fn execute(&self, app: &App, ctx: &CallContext) -> Result<Value, Error> {
        Ok(match self {
            Self::Create {
                operation_id,
                input,
            } => json!(
                app.create_structured(ctx, operation_id, input.clone())
                    .await?
            ),
            Self::Revise {
                operation_id,
                input,
            } => json!(
                app.revise_structured(ctx, operation_id, input.clone())
                    .await?
            ),
            Self::Get { id, revision } => json!(app.get_structured(ctx, id, *revision).await?),
            Self::History { id } => json!(app.structured_history(ctx, id).await?),
            Self::SetDeleted {
                operation_id,
                id,
                expected_revision,
                deleted,
            } => json!(
                app.set_structured_deleted(ctx, operation_id, id, *expected_revision, *deleted)
                    .await?
            ),
            Self::Source { id, revision } => app.structured_source(ctx, id, *revision).await?,
            Self::ConceptResolve { subject_id, alias } => {
                json!(app.concept_by_alias(ctx, subject_id, alias).await?)
            }
            Self::Query { input } => app.query_structured(ctx, input.clone()).await?,
            Self::Form {
                operation_id,
                input,
            } => json!(
                app.form_structured(ctx, operation_id, input.clone())
                    .await?
            ),
            Self::FormationRun { job_id } => app.structured_formation_run(ctx, job_id).await?,
            Self::MergeConcepts {
                operation_id,
                input,
            } => json!(app.merge_concepts(ctx, operation_id, input.clone()).await?),
            Self::SplitConcept {
                operation_id,
                input,
            } => json!(app.split_concept(ctx, operation_id, input.clone()).await?),
            Self::ResegmentEpisodes {
                operation_id,
                input,
            } => json!(
                app.resegment_episodes(ctx, operation_id, input.clone())
                    .await?
            ),
        })
    }
    pub fn request(&self) -> (&'static str, String, Option<Value>) {
        ("POST", "/v1/cognition".into(), Some(json!(self)))
    }
}
pub fn router() -> Router<ApiState> {
    Router::new().route("/v1/cognition", post(handle))
}
async fn handle(
    State(state): State<ApiState>,
    Extension(ctx): Extension<CallContext>,
    body: Result<Json<StructuredOperation>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    run(state, ctx, Operation::Structured(Box::new(decode(body)?))).await
}
