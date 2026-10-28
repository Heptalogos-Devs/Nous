mod record;
mod worker;
use crate::{App, import_format};
use nous_domain::*;
use nous_storage::{BeginOperation, OperationReceipt, WriteTransaction, operation_key};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
struct ImportPlan {
    caller_id: String,
    form_memory: bool,
    records: Vec<ParsedImportRecord>,
}
impl App {
    pub async fn submit_import(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: ImportRequest,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if let Some(origin) = &request.origin {
            origin.validate()?;
        }
        let mut records =
            import_format::parse(&request.content, request.format, request.source.as_deref())?;
        for parsed in &mut records {
            if let Some(record) = &mut parsed.record
                && record.origin.is_none()
            {
                record.origin = request.origin.clone();
            }
        }
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(ctx, "import.submit", &scope, operation_id, &request)?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        tx.subject(&request.namespace_id, &request.subject_id)
            .await?;
        let mut job = self.tasks.new_job(
            &tx,
            "import.text",
            Some(request.namespace_id),
            Some(request.subject_id),
        );
        let count = records.len();
        job.checkpoint = Some(json!(ImportPlan {
            caller_id: ctx.caller_id.clone(),
            form_memory: request.form_memory,
            records
        }));
        job.result = Some(json!(ImportResult {
            job_id: job.id.clone(),
            items: vec![],
            accepted: 0,
            replayed: 0,
            rejected: 0
        }));
        self.tasks.submit_in(&mut tx, &job).await?;
        tx.commit(json!({"job_id":job.id,"records":count})).await
    }

    pub async fn material_details(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<MaterialDetails> {
        Ok(self.get_material(ctx, id).await?.details)
    }

    async fn append_import_progress(
        &self,
        tx: &mut WriteTransaction,
        job: &mut JobRecord,
        parsed: &ParsedImportRecord,
        admitted: Option<(ImportAdmission, i64, bool)>,
        error: Option<Error>,
    ) -> Result<()> {
        let mut result = progress(job)?;
        if parsed.index != result.items.len() {
            return Err(Error::internal(
                "import record progress is not a contiguous prefix",
            ));
        }
        let (status, admission, commit_seq) = match admitted {
            Some((value, seq, true)) => {
                result.replayed += 1;
                ("replayed", Some(value), Some(seq))
            }
            Some((value, seq, false)) => {
                result.accepted += 1;
                ("accepted", Some(value), Some(seq))
            }
            None => {
                result.rejected += 1;
                ("rejected", None, None)
            }
        };
        result.items.push(ImportItemResult {
            index: parsed.index,
            external_id: parsed.external_id.clone(),
            status: status.into(),
            admission,
            commit_seq,
            error,
        });
        job.result = Some(json!(result));
        self.tasks
            .change_in(tx, job, JobState::Running, None, None)
            .await
    }

    async fn import_progress(
        &self,
        job: &JobRecord,
        parsed: &ParsedImportRecord,
        admitted: Option<(ImportAdmission, i64, bool)>,
        error: Option<Error>,
    ) -> Result<()> {
        let mut tx = self.tasks.operation("import.progress", &job.id).await?;
        let Some(mut current) = current_for(&mut tx, &job.id, parsed.index).await? else {
            tx.rollback().await?;
            return Ok(());
        };
        self.append_import_progress(&mut tx, &mut current, parsed, admitted, error)
            .await?;
        tx.commit(json!({"job_id":job.id,"record_index":parsed.index}))
            .await?;
        Ok(())
    }
}
fn progress(job: &JobRecord) -> Result<ImportResult> {
    let value: ImportResult = serde_json::from_value(
        job.result
            .clone()
            .ok_or_else(|| Error::internal("import task has no result prefix"))?,
    )?;
    if value.job_id != job.id {
        return Err(Error::internal("import progress belongs to another task"));
    }
    Ok(value)
}
async fn current_for(
    tx: &mut WriteTransaction,
    id: &str,
    index: usize,
) -> Result<Option<JobRecord>> {
    let job = tx.job_record(id).await?;
    if job.kind != "import.text" {
        return Err(Error::internal("import progress names another task kind"));
    }
    if job.state != JobState::Running || progress(&job)?.items.len() > index {
        return Ok(None);
    }
    if progress(&job)?.items.len() != index {
        return Err(Error::internal("import progress skipped a record"));
    }
    Ok(Some(job))
}
