use super::*;
use nous_domain::{SubjectId, context_work::*};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag="action",rename_all="snake_case",deny_unknown_fields)]
pub enum ContextOperation {
    CreateWork { operation_id:String, request:CreateWorkContextRequest },
    ReadWork { id:WorkContextId },
    ListWork { subject_id:SubjectId, limit:usize },
    WorkHistory { id:WorkContextId, limit:usize },
    ChangeWork { operation_id:String, id:WorkContextId, expected_revision:i64, change:WorkContextChange },
    OpenSession { operation_id:String, request:OpenContextSessionRequest },
    ReadSession { id:ContextSessionId },
    ChangeSession { operation_id:String,id:ContextSessionId,expected_revision:i64,change:ContextSessionChange },
    Residents { session_id:ContextSessionId, #[serde(default)] include_evicted:bool },
    ChangeResidents { operation_id:String,session_id:ContextSessionId,expected_revision:i64,change:ContextResidentChange },
    Prepare { operation_id:String, request:Box<ContextPrepareRequest> },
    Preparation { id:ContextPreparationId },
    CancelPreparation { operation_id:String,id:ContextPreparationId },
    Admit { operation_id:String,request:ContextAdmitRequest },
    Admission { id:ContextAdmissionId },
    Report { operation_id:String,request:ContextReportRequest },
}
impl ContextOperation {
    pub async fn execute(&self,app:&App,ctx:&CallContext)->Result<Value,Error> {
        Ok(match self {
            Self::CreateWork{operation_id,request}=>json!(app.create_work_context(ctx,operation_id,request.clone()).await?),
            Self::ReadWork{id}=>json!(app.get_work_context(ctx,id).await?),
            Self::ListWork{subject_id,limit}=>json!(app.list_work_contexts(ctx,subject_id,*limit).await?),
            Self::WorkHistory{id,limit}=>json!(app.work_context_history(ctx,id,*limit).await?),
            Self::ChangeWork{operation_id,id,expected_revision,change}=>json!(app.change_work_context(ctx,operation_id,id,*expected_revision,change.clone()).await?),
            Self::OpenSession{operation_id,request}=>json!(app.open_context_session(ctx,operation_id,request.clone()).await?),
            Self::ReadSession{id}=>json!(app.get_context_session(ctx,id).await?),
            Self::ChangeSession{operation_id,id,expected_revision,change}=>json!(app.change_context_session(ctx,operation_id,id,*expected_revision,change.clone()).await?),
            Self::Residents{session_id,include_evicted}=>json!(app.context_residents(ctx,session_id,*include_evicted).await?),
            Self::ChangeResidents{operation_id,session_id,expected_revision,change}=>json!(app.change_context_residents(ctx,operation_id,session_id,*expected_revision,change.clone()).await?),
            Self::Prepare{operation_id,request}=>json!(app.prepare_context(ctx,operation_id,*request.clone()).await?),
            Self::Preparation{id}=>json!(app.context_preparation(ctx,id).await?),
            Self::CancelPreparation{operation_id,id}=>json!(app.cancel_context_preparation(ctx,operation_id,id).await?),
            Self::Admit{operation_id,request}=>json!(app.admit_context(ctx,operation_id,request.clone()).await?),
            Self::Admission{id}=>json!(app.context_admission(ctx,id).await?),
            Self::Report{operation_id,request}=>json!(app.report_context(ctx,operation_id,request.clone()).await?),
        })
    }
}
pub(super) fn router()->Router<ApiState> {Router::new().route("/api/contexts",post(contexts))}
async fn contexts(State(state):State<ApiState>,Extension(ctx):Extension<CallContext>,body:Result<Json<ContextOperation>,JsonRejection>)->Result<Json<Value>,ApiError> {
    run(state,ctx,Operation::Context(Box::new(decode(body)?))).await
}
