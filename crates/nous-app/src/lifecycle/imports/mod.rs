mod input;
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
    inputs: Vec<ImportInputRef>,
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
        let scope = format!(
            "namespace:{}/subject:{}",
            request.namespace_id, request.subject_id
        );
        let key = operation_key(ctx, "import.submit", &scope, operation_id, &request)?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let catalog = self
            .freeze_import_inputs(ctx, &request, &CancellationToken::new())
            .await?;
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
        let (inputs, records) = self.register_import_inputs(&mut tx, &job, catalog).await?;
        let count = records.len();
        job.checkpoint = Some(json!(ImportPlan {
            caller_id: ctx.caller_id.clone(),
            form_memory: request.form_memory,
            inputs: inputs.clone(),
            records
        }));
        job.result = Some(json!(ImportResult {
            job_id: job.id.clone(),
            inputs: inputs.clone(),
            items: vec![],
            accepted: 0,
            replayed: 0,
            rejected: 0
        }));
        self.tasks.submit_in(&mut tx, &job).await?;
        tx.commit(json!({"job_id":job.id,"inputs":inputs,"records":count}))
            .await
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
        input_source: &ImportInputSourceId,
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
                self.append_import_container(tx, job, parsed, input_source, &value, true)
                    .await?;
                result.replayed += 1;
                ("replayed", Some(value), Some(seq))
            }
            Some((value, seq, false)) => {
                self.append_import_container(tx, job, parsed, input_source, &value, false)
                    .await?;
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
            input_index: Some(parsed.input_index),
            record_index: parsed.record_index,
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
        plan: &ImportPlan,
        parsed: &ParsedImportRecord,
        admitted: Option<(ImportAdmission, i64, bool)>,
        error: Option<Error>,
    ) -> Result<()> {
        let mut tx = self.tasks.operation("import.progress", &job.id).await?;
        let Some(mut current) = current_for(&mut tx, &job.id, parsed.index).await? else {
            tx.rollback().await?;
            return Ok(());
        };
        self.append_import_progress(
            &mut tx,
            &mut current,
            parsed,
            plan.input_source(parsed)?,
            admitted,
            error,
        )
        .await?;
        tx.commit(json!({"job_id":job.id,"record_index":parsed.index}))
            .await?;
        Ok(())
    }

    async fn append_import_container(
        &self,
        tx: &mut WriteTransaction,
        job: &JobRecord,
        parsed: &ParsedImportRecord,
        input_source: &ImportInputSourceId,
        admission: &ImportAdmission,
        replayed: bool,
    ) -> Result<()> {
        let (namespace, subject) = job.subject_scope()?;
        tx.insert_import_record_container(&ImportRecordContainer {
            id: ImportRecordContainerId::new(),
            namespace_id: namespace.clone(),
            subject_id: subject.clone(),
            input_source_id: input_source.clone(),
            record_index: parsed
                .record_index
                .ok_or_else(|| Error::internal("admitted record has no original input position"))?,
            record_material_id: admission.material_id.clone(),
            observation_id: admission.observation_id.clone(),
            memory_id: admission.memory_id.clone(),
            replayed,
            original_job_id: job.id.clone(),
            original_attempt_index: parsed.index,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        })
        .await
    }
}
impl ImportPlan {
    fn input_source(&self, record: &ParsedImportRecord) -> Result<&ImportInputSourceId> {
        self.inputs
            .get(record.input_index)
            .filter(|input| input.input_index == record.input_index)
            .map(|input| &input.source_id)
            .ok_or_else(|| Error::internal("import record leaves its frozen input catalog"))
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
