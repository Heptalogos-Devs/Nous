use super::*;
use nous_foundation::{
    model_calls::{AdmissionDecision, AdmissionFuture, CallAdmission},
    models::{InvocationMetadata, ModelFailure},
};
use tokio_util::sync::CancellationToken;

struct Admission;
impl CallAdmission<model::Metadata> for Admission {
    fn validate<'a>(
        &'a self,
        tx: &'a mut nous_storage::WriteTransaction,
        metadata: &'a model::Metadata,
    ) -> AdmissionFuture<'a> {
        Box::pin(async move {
            let (namespace, subject) = match &metadata.plan.scope {
                ConfigScope::Subject {
                    namespace_id,
                    subject_id,
                } => (namespace_id.clone(), subject_id.clone()),
                _ => return Err(Error::internal("rerank requires subject scope")),
            };
            for document in &metadata.input.documents {
                tx.require_query_target(
                    &namespace,
                    &subject,
                    &document.reference,
                    document.current_head,
                )
                .await?;
            }
            Ok(AdmissionDecision::Ready)
        })
    }
}
impl App {
    pub(crate) fn register_rerank_tasks(
        &self,
        registry: &mut nous_foundation::tasks::TaskRegistry,
    ) -> Result<()> {
        let owner = self.clone();
        let readiness = self.clone();
        registry.register(
            "query.rerank",
            move |job, cancel| {
                let owner = owner.clone();
                async move { owner.run_rerank_job(job, cancel).await }
            },
            move |job| {
                let owner = readiness.clone();
                async move { owner.rerank_ready(&job).await }
            },
        )
    }
    pub(crate) async fn run_rerank_job(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let run = checkpoint(&job)?;
        let (namespace, subject) = job.subject_scope()?;
        let ctx = job.call_context("nous-worker")?;
        let plan = self.model.plan(
            ConfigScope::Subject {
                namespace_id: namespace.clone(),
                subject_id: subject.clone(),
            },
            model::ROLE,
            cancel.clone(),
        );
        let Some(call) = self
            .calls
            .invoke(
                &job.id,
                "rerank",
                &cancel,
                |attempt| {
                    let plan = &plan;
                    let run = &run;
                    let ctx = &ctx;
                    async move {
                        let input = self
                            .rerank_input(ctx, run)
                            .await
                            .map_err(ModelFailure::domain)?;
                        plan.prepare(attempt, input, Default::default()).await
                    }
                },
                |prepared| model::execute(&self.model, prepared),
                Some(&Admission),
            )
            .await?
        else {
            return Ok(());
        };
        let mut tx = self.tasks.operation("query.rerank.adopt", &job.id).await?;
        let mut current = tx.job_record(&job.id).await?;
        if matches!(
            current.state,
            JobState::Succeeded | JobState::Cancelled | JobState::Failed
        ) {
            tx.rollback().await?;
            return Ok(());
        }
        let call = tx.model_call(&call.id).await?;
        if call.job_id != job.id
            || call.step_key != "rerank"
            || call.state != ModelCallState::Succeeded
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "rerank call does not belong to this successful work step",
            ));
        }
        let metadata: InvocationMetadata<model::RerankInput> =
            serde_json::from_value(call.request.clone())?;
        if metadata.input.documents.len() != run.candidates.len() {
            return Err(Error::internal("rerank candidate catalog changed"));
        }
        for (document, accepted) in metadata.input.documents.iter().zip(&run.candidates) {
            if document.reference != accepted.reference
                || document.text.len() != accepted.sent_bytes
                || capture::digest(&document.text) != accepted.sent_sha256
            {
                return Err(Error::internal(
                    "rerank paid input differs from its accepted catalog",
                ));
            }
            tx.require_query_target(
                namespace,
                subject,
                &accepted.reference,
                accepted.current_head,
            )
            .await?;
        }
        let scores: Vec<nous_foundation::models::RerankScore> = serde_json::from_value(
            call.response
                .as_ref()
                .and_then(|response| response.get("result"))
                .cloned()
                .ok_or_else(|| Error::internal("rerank call result missing"))?,
        )?;
        self.tasks.complete_in(&mut tx,&mut current,json!({"call_id":call.id,"scores":scores,"references":run.candidates.iter().map(|candidate|&candidate.reference).collect::<Vec<_>>()})).await?;
        tx.commit(json!({"job_id":job.id,"model_call_id":call.id}))
            .await?;
        Ok(())
    }
    pub(crate) async fn rerank_ready(&self, job: &JobRecord) -> bool {
        let Ok(ctx) = job.call_context("nous-worker") else {
            return false;
        };
        let Ok(run) = checkpoint(job) else {
            return true;
        };
        // A vanished source should run once and fail, rather than wait for model configuration forever.
        let Ok(input) = self.rerank_input(&ctx, &run).await else {
            return true;
        };
        self.calls
            .ready(
                job,
                "rerank",
                &self.model,
                model::ROLE,
                serde_json::to_value(input).ok(),
            )
            .await
    }
}
fn checkpoint(job: &JobRecord) -> Result<capture::RerankRun> {
    serde_json::from_value(
        job.checkpoint
            .clone()
            .ok_or_else(|| Error::internal("rerank task has no accepted catalog"))?,
    )
    .map_err(Error::from)
}
