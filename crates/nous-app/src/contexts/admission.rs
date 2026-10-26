use super::*;
use nous_foundation::{
    model_calls::{AdmissionDecision, AdmissionFuture, CallAdmission},
    models::InvocationMetadata,
};
use nous_storage::WriteTransaction;
pub(super) struct StewardAdmission(pub ContextPreparationId);
impl CallAdmission<InvocationMetadata<model::StewardInput>> for StewardAdmission {
    fn validate<'a>(
        &'a self,
        tx: &'a mut WriteTransaction,
        metadata: &'a InvocationMetadata<model::StewardInput>,
    ) -> AdmissionFuture<'a> {
        Box::pin(async move {
            let preparation = tx.context_preparation(&self.0).await?;
            if preparation.response.is_some()
                || matches!(
                    preparation.job.state,
                    JobState::Succeeded | JobState::Failed | JobState::Cancelled
                )
            {
                return Err(Error::new(
                    ErrorCode::Cancelled,
                    "context no longer requires Steward execution",
                ));
            }
            if serde_json::to_value(model::input(&preparation))?
                != serde_json::to_value(&metadata.input)?
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Steward input differs from the accepted context catalog",
                ));
            }
            let (ns, subject) = preparation.job.subject_scope()?;
            let session = tx.context_session(&preparation.request.session_id).await?;
            if session.history_invalidated
                || session.closed_at.is_some()
                || Some(session.revision) != preparation.session_revision
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "context Session changed before Steward reservation",
                ));
            }
            if let Some(context) = &preparation.work_context {
                let head = tx.work_context(&context.id).await?;
                if head.revision != context.revision || head.state != WorkContextState::Open {
                    return Err(Error::new(
                        ErrorCode::Conflict,
                        "foreground WorkContext changed before Steward reservation",
                    ));
                }
            }
            for segment in &preparation.catalog {
                tx.validate_context_targets(ns, subject, &segment.sources)
                    .await?;
            }
            Ok(AdmissionDecision::Ready)
        })
    }
}
