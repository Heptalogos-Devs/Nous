mod adoption;
mod compile;
mod models;
mod time;
mod unit;
use crate::App;
use nous_domain::*;
use nous_foundation::tasks::{DependencyResolution, TaskRegistry};
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParentPlan {
    request: ProcessImportUnitsRequest,
    caller_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UnitPlan {
    catalog_id: ImportUnitCatalogId,
    unit_id: ImportUnitId,
    parent_job_id: String,
    caller_id: String,
    llm_enabled: bool,
    extract_time: bool,
    vectors: ImportVectorPolicy,
    origin: Option<ObservationOrigin>,
}

impl App {
    pub async fn import_unit_output(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        id: &ImportUnitOutputId,
    ) -> Result<ImportUnitOutput> {
        ctx.require_subject(namespace, subject)?;
        let mut view = self.database.read_view().await?;
        let output = view.import_unit_output(namespace, subject, id).await?;
        let unit = view
            .import_text_unit(namespace, subject, &output.unit_id)
            .await?;
        view.require_source_requirements(ctx, namespace, subject, &unit.source_requirements())
            .await?;
        view.finish().await?;
        Ok(output)
    }
    pub async fn process_import_units(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        mut request: ProcessImportUnitsRequest,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        if let Some(origin) = &request.origin {
            origin.validate()?;
        }
        let key = operation_key(
            ctx,
            "import.process",
            &format!(
                "namespace:{}/subject:{}",
                request.namespace_id, request.subject_id
            ),
            operation_id,
            &request,
        )?;
        if let Some(receipt) = self.database.lookup_operation(&key).await? {
            return Ok(receipt);
        }
        let ids = if let Some(ids) = &request.unit_ids {
            ids.clone()
        } else {
            self.import_catalog(
                ctx,
                ReadImportCatalogRequest {
                    namespace_id: request.namespace_id.clone(),
                    subject_id: request.subject_id.clone(),
                    catalog_id: request.catalog_id.clone(),
                },
                cancel,
            )
            .await?
            .units
            .into_iter()
            .map(|unit| unit.id)
            .collect()
        };
        let inputs = self
            .retry_import_inputs(
                ctx,
                RetryImportInputsRequest {
                    namespace_id: request.namespace_id.clone(),
                    subject_id: request.subject_id.clone(),
                    catalog_id: request.catalog_id.clone(),
                    unit_ids: ids.clone(),
                },
                cancel,
            )
            .await?;
        if let Some(parent_id) = &request.retry_parent_job_id {
            let parent = self.tasks.read(ctx, parent_id).await?;
            let (namespace, subject) = parent.subject_scope()?;
            if parent.kind != "import.process"
                || namespace != &request.namespace_id
                || subject != &request.subject_id
            {
                return Err(Error::invalid(
                    "retry parent must be an import processing task in this subject",
                ));
            }
            let parent_plan: ParentPlan = serde_json::from_value(
                parent
                    .checkpoint
                    .ok_or_else(|| Error::internal("retry parent has no frozen request"))?,
            )?;
            if parent_plan.request.catalog_id != request.catalog_id {
                return Err(Error::invalid(
                    "retry must retain its parent's frozen catalog",
                ));
            }
            let result: ImportProcessingResult = serde_json::from_value(
                parent
                    .result
                    .ok_or_else(|| Error::invalid("retry parent has no actual unit tasks"))?,
            )?;
            for unit in &inputs.units {
                let reference = result
                    .unit_jobs
                    .iter()
                    .find(|item| item.unit_id == unit.id)
                    .ok_or_else(|| Error::invalid("retry unit was not attempted by its parent"))?;
                if self.tasks.read(ctx, &reference.job_id).await?.state != JobState::Failed {
                    return Err(Error::invalid(
                        "retry selection must contain actual failed unit tasks",
                    ));
                }
            }
        }
        if inputs.units.is_empty() {
            return Err(Error::invalid(
                "processing requires at least one retained unit",
            ));
        }
        request.unit_ids = Some(ids);
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        let mut job = self.tasks.new_job(
            &tx,
            "import.process",
            Some(request.namespace_id.clone()),
            Some(request.subject_id.clone()),
        );
        job.checkpoint = Some(json!(ParentPlan {
            request,
            caller_id: ctx.caller_id.clone()
        }));
        self.tasks.submit_in(&mut tx, &job).await?;
        tx.commit(json!({"job_id":job.id})).await
    }

    pub(crate) fn register_import_processing_tasks(
        &self,
        registry: &mut TaskRegistry,
    ) -> Result<()> {
        let parent = self.clone();
        registry.register(
            "import.process",
            move |job, cancel| {
                let app = parent.clone();
                async move { app.run_import_processing(job, cancel).await }
            },
            |_| async { true },
        )?;
        let unit = self.clone();
        let ready = self.clone();
        registry.register(
            "import.unit",
            move |job, cancel| {
                let app = unit.clone();
                async move { app.run_import_unit(job, cancel).await }
            },
            move |job| {
                let app = ready.clone();
                async move { app.import_unit_ready(&job).await }
            },
        )?;
        self.register_import_time_task(registry)
    }

    async fn run_import_processing(&self, job: JobRecord, cancel: CancellationToken) -> Result<()> {
        let plan: ParentPlan = serde_json::from_value(
            job.checkpoint
                .clone()
                .ok_or_else(|| Error::internal("processing task has no frozen request"))?,
        )?;
        let request = &plan.request;
        if job.result.is_none() {
            let ctx = job.call_context(&plan.caller_id)?;
            let inputs = self
                .retry_import_inputs(
                    &ctx,
                    RetryImportInputsRequest {
                        namespace_id: request.namespace_id.clone(),
                        subject_id: request.subject_id.clone(),
                        catalog_id: request.catalog_id.clone(),
                        unit_ids: request.unit_ids.clone().ok_or_else(|| {
                            Error::internal("processing has no frozen unit selection")
                        })?,
                    },
                    &cancel,
                )
                .await?;
            let mut tx = self
                .tasks
                .operation("import.process.children", &job.id)
                .await?;
            let mut current = tx.job_record(&job.id).await?;
            if current.state != JobState::Running {
                tx.rollback().await?;
                return Ok(());
            }
            if current.result.is_some() {
                tx.rollback().await?;
                return Ok(());
            }
            let mut references = Vec::new();
            for input in inputs.units {
                let mut child = self.tasks.new_job(
                    &tx,
                    "import.unit",
                    job.namespace_id.clone(),
                    job.subject_id.clone(),
                );
                child.source_read.push(SourceReadScope {
                    subject_id: request.subject_id.clone(),
                    source_refs: input.source_requirements(),
                    origin_conversation_refs: vec![],
                });
                child.checkpoint = Some(json!(UnitPlan {
                    catalog_id: request.catalog_id.clone(),
                    unit_id: input.id.clone(),
                    parent_job_id: job.id.clone(),
                    caller_id: plan.caller_id.clone(),
                    llm_enabled: request.llm_enabled,
                    extract_time: request.extract_time,
                    vectors: request.vectors,
                    origin: request.origin.clone(),
                }));
                self.tasks
                    .submit_child_in(&mut tx, &job.id, &mut child)
                    .await?;
                references.push(ImportUnitJobRef {
                    unit_id: input.id,
                    job_id: child.id,
                });
            }
            current.result = Some(json!(ImportProcessingResult {
                catalog_id: request.catalog_id.clone(),
                unit_jobs: references.clone()
            }));
            self.tasks
                .suspend_until_in(
                    &mut tx,
                    &mut current,
                    references.into_iter().map(|item| item.job_id).collect(),
                    DependencyPolicy::AllTerminal,
                )
                .await?;
            tx.commit(json!({"job_id":job.id})).await?;
            return Ok(());
        }
        let mut tx = self
            .tasks
            .operation("import.process.finish", &job.id)
            .await?;
        let mut current = tx.job_record(&job.id).await?;
        if current.state != JobState::Running {
            tx.rollback().await?;
            return Ok(());
        }
        let dependencies = current.dependencies.clone();
        if self
            .tasks
            .suspend_until_in(
                &mut tx,
                &mut current,
                dependencies,
                DependencyPolicy::AllTerminal,
            )
            .await?
            != DependencyResolution::Ready
        {
            tx.commit(json!({"job_id":job.id})).await?;
            return Ok(());
        }
        let result: ImportProcessingResult = serde_json::from_value(
            current
                .result
                .clone()
                .ok_or_else(|| Error::internal("processing has no actual unit jobs"))?,
        )?;
        let mut failed = Vec::new();
        for reference in &result.unit_jobs {
            let child = tx.job_record(&reference.job_id).await?;
            if child.state != JobState::Succeeded {
                failed.push(child.id);
            }
        }
        if failed.is_empty() {
            self.tasks
                .complete_in(&mut tx, &mut current, json!(result))
                .await?;
        } else {
            self.tasks
                .change_in(
                    &mut tx,
                    &mut current,
                    JobState::Failed,
                    None,
                    Some(
                        Error::new(
                            ErrorCode::Conflict,
                            "one or more import units did not complete",
                        )
                        .with_context("unit_jobs", json!(failed)),
                    ),
                )
                .await?;
        }
        tx.commit(json!({"job_id":job.id})).await?;
        Ok(())
    }
}
