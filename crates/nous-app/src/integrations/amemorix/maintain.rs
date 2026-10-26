use super::{graph::step, types::*};
use crate::App;
use nous_domain::*;
use nous_storage::{BeginOperation, operation_key};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize, Deserialize)]
struct Plan {
    action: MaintainAction,
    hours: Option<f64>,
    reason: String,
    targets: Vec<CognitiveRef>,
    receipts: Vec<AppliedChange>,
}
#[derive(Clone, Serialize, Deserialize)]
struct AppliedChange {
    reference: CognitiveRef,
    operation_id: String,
    commit_seq: i64,
}
pub(super) async fn execute(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &MaintainInput,
) -> Result<Value> {
    if matches!(input.action, MaintainAction::RecycleBin) {
        let rows = app
            .recycle_bin(ctx, &binding.subject_id, input.limit)
            .await?;
        let mut items = Vec::new();
        for memory in rows {
            match app
                .database
                .require_source_read(
                    ctx,
                    &binding.namespace_id,
                    &binding.subject_id,
                    &RetrievalTarget::Cognition(CognitiveRef::memory(&memory.id, memory.revision)),
                )
                .await {
                Ok(())=>items.push(json!({"hash":memory.id,"content":memory.content,"type":"paragraph","memory":memory})),
                Err(error) if matches!(error.code,ErrorCode::Forbidden|ErrorCode::NotFound)=>{},
                Err(error)=>return Err(error),
            }
        }
        return Ok(json!({"success":true,"count":items.len(),"items":items}));
    }
    if input.hours.is_some() && !matches!(input.action, MaintainAction::Protect) {
        return Err(Error::invalid("hours applies to protect"));
    }
    let scope = format!(
        "namespace:{}/subject:{}",
        binding.namespace_id, binding.subject_id
    );
    let key = operation_key(
        ctx,
        "amemorix.maintain",
        &scope,
        &input.operation_id,
        &json!(input),
    )?;
    if let Some(receipt) = app.database.lookup_operation(&key).await? {
        return Ok(receipt.result);
    }
    let mut targets = Vec::new();
    if let Ok(reference) = input.target.parse::<CognitiveRef>() {
        targets.push(reference);
    } else if let Ok(id) = input.target.parse::<MemoryId>() {
        let memory = app.database.memory(&binding.namespace_id, &id).await?;
        if memory.subject_id != binding.subject_id {
            return Err(Error::new(
                ErrorCode::Forbidden,
                "maintenance target belongs to another subject",
            ));
        }
        targets.push(CognitiveRef::memory(&id, memory.revision));
    } else if !input.target.trim().is_empty() {
        let query: QueryRequest = serde_json::from_value(
            json!({"subject_id":binding.subject_id,"mode":"lexical","input":{"text":input.target},"kinds":["memory"],"limit":input.limit,"include_deleted":matches!(input.action,MaintainAction::Restore),"include_inactive":true}),
        )?;
        targets = app
            .query(ctx, query)
            .await?
            .hits
            .into_iter()
            .filter_map(|hit| hit.reference.cognition().cloned())
            .collect();
    }
    for reference in &targets {
        if reference.kind != CognitiveKind::Memory {
            return Err(Error::new(
                ErrorCode::CapabilityUnavailable,
                "this lifecycle action requires a Memory target",
            )
            .with_context("reference", reference.to_string()));
        }
        app.database
            .require_source_read(
                ctx,
                &binding.namespace_id,
                &binding.subject_id,
                &RetrievalTarget::Cognition(reference.clone()),
            )
            .await?;
    }
    if targets.is_empty() {
        return Ok(json!({"success":false,"detail":"no_matching_target","targets":[]}));
    }
    let mut tx = match app.database.begin_operation(key).await? {
        BeginOperation::Replay(receipt) => return Ok(receipt.result),
        BeginOperation::New(tx) => tx,
    };
    let mut job = app.tasks.new_job(
        &tx,
        "amemorix.maintain",
        Some(binding.namespace_id.clone()),
        Some(binding.subject_id.clone()),
    );
    job.checkpoint = Some(json!(Plan {
        action: input.action,
        hours: input.hours,
        reason: input.reason.clone(),
        targets,
        receipts: vec![]
    }));
    app.tasks.submit_in(&mut tx, &job).await?;
    Ok(tx
        .commit(json!({"accepted":true,"job_id":job.id,"job_ids":[job.id]}))
        .await?
        .result)
}
impl App {
    pub(crate) fn register_amemorix_tasks(
        &self,
        registry: &mut nous_foundation::tasks::TaskRegistry,
    ) -> Result<()> {
        let owner = self.clone();
        registry.register(
            "amemorix.maintain",
            move |job, cancel| {
                let owner = owner.clone();
                async move { owner.run_amemorix_maintenance(job, cancel).await }
            },
            |_| async { true },
        )
    }
    async fn run_amemorix_maintenance(
        &self,
        job: JobRecord,
        cancel: CancellationToken,
    ) -> Result<()> {
        let ctx = job.call_context("nous-worker")?;
        let mut plan: Plan = serde_json::from_value(
            job.checkpoint
                .clone()
                .ok_or_else(|| Error::internal("maintenance task has no accepted targets"))?,
        )?;
        for index in plan.receipts.len()..plan.targets.len() {
            nous_foundation::execution::check_cancelled(&cancel)?;
            let current = self.database.job_record(&job.id).await?;
            if matches!(
                current.state,
                JobState::Cancelled | JobState::Succeeded | JobState::Failed
            ) {
                return Ok(());
            }
            let target = &plan.targets[index];
            let operation_id = step(&job.id, &target.to_string());
            let receipt = self
                .change_memory(
                    &ctx,
                    &operation_id,
                    &MemoryId(target.id.clone()),
                    MemoryChange {
                        action: action(plan.action)?,
                        expected_revision: target.revision,
                        strength: None,
                        hours: plan.hours,
                        reason: plan.reason.clone(),
                        force: false,
                        edit: None,
                    },
                )
                .await?;
            let revision = receipt
                .result
                .get("revision")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    Error::internal("memory lifecycle receipt has no resulting revision")
                })?;
            plan.receipts.push(AppliedChange {
                reference: CognitiveRef::memory(&MemoryId(target.id.clone()), revision),
                operation_id,
                commit_seq: receipt.commit_seq,
            });
            let mut tx = self
                .tasks
                .operation("amemorix.maintain.progress", &job.id)
                .await?;
            let mut current = tx.job_record(&job.id).await?;
            if matches!(
                current.state,
                JobState::Cancelled | JobState::Succeeded | JobState::Failed
            ) {
                tx.rollback().await?;
                return Ok(());
            }
            current.checkpoint = Some(json!(plan));
            self.tasks
                .change_in(&mut tx, &mut current, JobState::Running, None, None)
                .await?;
            tx.commit(json!({"completed":plan.receipts.len()})).await?;
        }
        let mut tx = self
            .tasks
            .operation("amemorix.maintain.completed", &job.id)
            .await?;
        let mut current = tx.job_record(&job.id).await?;
        if matches!(
            current.state,
            JobState::Cancelled | JobState::Succeeded | JobState::Failed
        ) {
            tx.rollback().await?;
            return Ok(());
        }
        self.tasks.complete_in(&mut tx,&mut current,json!({"success":true,"detail":"maintained","count":plan.receipts.len(),"receipts":plan.receipts})).await?;
        tx.commit(json!({"state":current.state})).await?;
        Ok(())
    }
}
fn action(action: MaintainAction) -> Result<MemoryAction> {
    Ok(match action {
        MaintainAction::Reinforce => MemoryAction::Reinforce,
        MaintainAction::Weaken => MemoryAction::Weaken,
        MaintainAction::Protect => MemoryAction::Protect,
        MaintainAction::Unprotect => MemoryAction::Unprotect,
        MaintainAction::Freeze => MemoryAction::Freeze,
        MaintainAction::Forget => MemoryAction::Forget,
        MaintainAction::Restore => MemoryAction::Restore,
        MaintainAction::Trash => MemoryAction::Trash,
        MaintainAction::Purge => MemoryAction::Purge,
        MaintainAction::Decay => MemoryAction::Decay,
        MaintainAction::RecycleBin => {
            return Err(Error::internal(
                "recycle-bin read is not a maintenance mutation",
            ));
        }
    })
}
