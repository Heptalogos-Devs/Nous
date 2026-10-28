pub mod config;
mod input;
mod presentation;
mod task;
use super::AmemorixRequest;
use crate::App;
use futures::{FutureExt, future::BoxFuture};
pub use input::ImportInput;
use nous_domain::*;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

pub(super) fn execute<'a>(
    app: &'a App,
    ctx: &'a CallContext,
    binding: &'a AmemorixRequest,
    input: &'a ImportInput,
) -> BoxFuture<'a, Result<Value>> {
    match input.action.trim().to_ascii_lowercase().as_str() {
        "settings"|"get_settings"|"get_guide"=>settings(app,binding).boxed(),
        "create_paste"|"create_upload"|"create_raw_scan"=>create(app,ctx,binding,input).boxed(),
        "list"=>list(app,ctx,binding,input).boxed(),
        "get"=>get(app,ctx,binding,input).boxed(),
        "chunks"|"get_chunks"=>chunks(app,ctx,binding,input).boxed(),
        "cancel"=>cancel(app,ctx,binding,input).boxed(),
        "retry_failed"=>retry(app,ctx,binding,input).boxed(),
        _=>async move {Err(Error::new(ErrorCode::CapabilityUnavailable,"import action requires its native converter, maintenance, or caller file-boundary owner").with_context("action",input.action.clone()))}.boxed(),
    }
}
fn decode<T: serde::de::DeserializeOwned>(input: &ImportInput) -> Result<T> {
    Ok(serde_json::from_value(Value::Object(
        input.parameters.clone(),
    ))?)
}
async fn settings(app: &App, binding: &AmemorixRequest) -> Result<Value> {
    let snapshot = app
        .configuration
        .snapshot(
            &ConfigScope::Subject {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
            },
            false,
        )
        .await?;
    let policy = snapshot.get(super::policy::AMEMORIX)?.imports;
    let split = snapshot.get(crate::lifecycle::imports::text::policy::IMPORT_TEXT)?;
    let tasks = snapshot.get(nous_foundation::tasks::TASK_RUNTIME)?;
    Ok(
        json!({"success":true,"settings":{"enabled":policy.enabled,"poll_interval_ms":policy.poll_interval_ms,
        "max_queue_size":null,"max_files_per_task":crate::import_format::MAX_IMPORT_RECORDS,"max_file_size_mb":crate::import_format::MAX_IMPORT_BYTES/1024/1024,"max_paste_chars":null,
        "default_narrative_window_size":split.narrative_window_size,"default_narrative_overlap":split.narrative_overlap,"default_factual_target_size":split.factual_target_size,
        "max_chunk_chars":null,"default_file_concurrency":null,"default_chunk_concurrency":null,"max_file_concurrency":null,"max_chunk_concurrency":null,
        "llm_enabled":policy.llm_enabled,"vectors":policy.vectors,"supported_input_modes":["text"],"path_aliases":null,"shared_task_concurrency":tasks.concurrency,
        "capabilities":{"request_concurrency":false,"file_dedupe":true,"json_processing":false,"unit_retry":true},
        "native_limits":{"input_bytes":crate::import_format::MAX_IMPORT_BYTES,"units":crate::import_format::MAX_IMPORT_RECORDS}},
        "content":"纯文本可上传、粘贴或从宿主受管目录选择。处理采用冻结 UnitId，任务、文件与分块状态来自真实子任务。时间抽取保留原始结果，当前仅带明确 offset 的 RFC3339 时间可归一化；日期或时区未知时保留 temporal_error，不补本机日期或 UTC。文件去重支持 content_hash/manifest/none；force 绕过本批命中，clear_manifest 清本作用域当前资格，历史保留。请求级并发由统一任务配置管理。JSON、LPMM 转换、时序回填与 MaiBot 数据库迁移分别由其专门处理能力接入。"}),
    )
}

async fn create(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let options: input::Create = decode(input)?;
    let action = input.action.trim().to_ascii_lowercase();
    let policy = app
        .configuration
        .snapshot(
            &ConfigScope::Subject {
                namespace_id: binding.namespace_id.clone(),
                subject_id: binding.subject_id.clone(),
            },
            false,
        )
        .await?
        .get(super::policy::AMEMORIX)?
        .imports;
    if !policy.enabled {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "import creation is disabled by configuration",
        ));
    }
    if options.file_concurrency.is_some() || options.chunk_concurrency.is_some() {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "request file/chunk concurrency requires the native scheduler's matching request policy",
        ));
    }
    let dedupe = options.dedupe_policy.as_deref().unwrap_or(if action == "create_raw_scan" { "manifest" } else { "content_hash" });
    let dedupe_policy: ImportFileDedupePolicy = serde_json::from_value(json!(dedupe.trim().to_ascii_lowercase()))?;
    if options
        .input_mode
        .as_deref()
        .is_some_and(|value| value != "text")
    {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "JSON import requires its record parser and semantic-unit owner",
        ));
    }
    let mut documents = options.inputs.clone();
    if action == "create_paste" {
        if !documents.is_empty() {
            return Err(Error::invalid(
                "paste accepts content, not an uploaded input catalog",
            ));
        }
        documents.push(ImportFileDocument { document: ImportDocument {
            name: options.name.clone(), format: ImportFormat::Text,
            input: ImportDocumentInput::Inline { content: options.content.clone().ok_or_else(|| Error::invalid("paste content is required"))? }, source: None,
        }, file_identity: None });
    }
    if documents.is_empty() {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "file creation requires the SDK's caller-owned material input catalog",
        ));
    }
    if documents
        .iter()
        .any(|document| document.document.format != ImportFormat::Text)
    {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "non-text documents retain their own parser and processing contract",
        ));
    }
    let operation_id = options
        .operation_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let origin = options.origin()?;
    let cancel = CancellationToken::new();
    let _guard = cancel.clone().drop_guard();
    let receipt = app.admit_import_files(ctx, &operation_id, AdmitImportFilesRequest {
        namespace_id: binding.namespace_id.clone(), subject_id: binding.subject_id.clone(), inputs: documents,
        overrides: Some(options.overrides()?), llm_enabled: options.llm_enabled.unwrap_or(policy.llm_enabled),
        extract_time: options.chat_log, vectors: options.vectors.unwrap_or(policy.vectors), origin,
        dedupe_policy, force: options.force, clear_manifest: options.clear_manifest,
    }, &cancel).await?;
    let admission: ImportFilesAdmission = serde_json::from_value(receipt.result.clone())?;
    let Some(id) = admission.processing_job_id.as_deref() else {
        return Ok(json!({"success":true,"skipped":true,"task":null,"files":admission.inputs,"operation_id":operation_id,"native":receipt}));
    };
    let task = task::read(app, ctx, binding, id, &cancel).await?;
    Ok(
        json!({"success":true,"skipped":false,"files":admission.inputs,"task":presentation::summary(&task,false,false)?,"operation_id":operation_id,"native":receipt}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    task_id: String,
    #[serde(default)]
    include_chunks: bool,
}
async fn get(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let request: Get = decode(input)?;
    let cancel = CancellationToken::new();
    match task::read(app, ctx, binding, &request.task_id, &cancel).await {
        Ok(task) => Ok(
            json!({"success":true,"task":presentation::summary(&task,true,request.include_chunks)?,"error":""}),
        ),
        Err(error) if error.code == ErrorCode::NotFound => {
            Ok(json!({"success":false,"task":null,"error":error.message}))
        }
        Err(error) => Err(error),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    #[serde(default = "list_limit")]
    limit: usize,
    cursor: Option<JobCursor>,
}
fn list_limit() -> usize {
    50
}
async fn list(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let request: List = decode(input)?;
    let page = app
        .jobs(
            ctx,
            &JobListRequest {
                namespace_id: Some(binding.namespace_id.clone()),
                subject_id: Some(binding.subject_id.clone()),
                kind: Some("import.process".into()),
                state: None,
                limit: request.limit.clamp(1, 200),
                cursor: request.cursor,
            },
        )
        .await?;
    let cancel = CancellationToken::new();
    let mut items = Vec::new();
    for job in &page.items {
        let task = task::read(app, ctx, binding, &job.id, &cancel).await?;
        items.push(presentation::summary(&task, false, false)?);
    }
    Ok(json!({"success":true,"count":items.len(),"items":items,"next_cursor":page.next_cursor}))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunks {
    task_id: String,
    file_id: ImportInputSourceId,
    #[serde(default)]
    offset: usize,
    #[serde(default = "list_limit")]
    limit: usize,
}
async fn chunks(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let request: Chunks = decode(input)?;
    let cancel = CancellationToken::new();
    let task = task::read(app, ctx, binding, &request.task_id, &cancel).await?;
    let Some(file) = task
        .inputs
        .iter()
        .find(|source| source.id == request.file_id)
    else {
        return Ok(json!({"success":false,"error":"任务中没有此输入文件"}));
    };
    let units = task
        .units
        .iter()
        .filter(|unit| unit.unit.input_source_id == request.file_id)
        .collect::<Vec<_>>();
    let limit = request.limit.clamp(1, 500);
    let items = units
        .iter()
        .skip(request.offset)
        .take(limit)
        .map(|unit| presentation::chunk(unit))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"success":true,"task_id":request.task_id,"file_id":request.file_id,"offset":request.offset,"limit":limit,"total":units.len(),"file":presentation::file(&task,file,false)?,"items":items,"error":""}),
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    task_id: String,
    operation_id: Option<String>,
}
async fn cancel(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let request: Cancel = decode(input)?;
    let cancel = CancellationToken::new();
    task::read(app, ctx, binding, &request.task_id, &cancel).await?;
    let operation_id = request
        .operation_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let receipt = app.cancel_job(ctx, &operation_id, &request.task_id).await?;
    let task = task::read(app, ctx, binding, &request.task_id, &cancel).await?;
    Ok(
        json!({"success":true,"task":presentation::summary(&task,false,false)?,"operation_id":operation_id,"native":receipt}),
    )
}
#[derive(Deserialize)]
struct Retry {
    task_id: String,
    operation_id: Option<String>,
    overrides: Option<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    parameters: serde_json::Map<String, Value>,
}
async fn retry(
    app: &App,
    ctx: &CallContext,
    binding: &AmemorixRequest,
    input: &ImportInput,
) -> Result<Value> {
    let request: Retry = decode(input)?;
    let cancel = CancellationToken::new();
    let task = task::read(app, ctx, binding, &request.task_id, &cancel).await?;
    if request.overrides.is_some() && !request.parameters.is_empty() {
        return Err(Error::invalid(
            "supply retry overrides either nested or as direct fields",
        ));
    }
    let overrides = request.overrides.unwrap_or(request.parameters);
    let changes = retry_overrides(&task, &overrides)?;
    let ids = task
        .units
        .iter()
        .filter(|unit| {
            unit.job
                .as_ref()
                .is_some_and(|job| job.state == JobState::Failed)
        })
        .map(|unit| unit.unit.id.clone())
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(Error::invalid("task has no actual failed unit attempts"));
    }
    let mut processing = task.request;
    if let Some(enabled) = changes.llm_enabled {
        processing.llm_enabled = enabled;
    }
    if let Some(vectors) = changes.vectors {
        processing.vectors = vectors;
    }
    processing.unit_ids = Some(ids);
    processing.retry_parent_job_id = Some(request.task_id);
    let operation_id = request
        .operation_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let receipt = app
        .process_import_units(ctx, &operation_id, processing, &cancel)
        .await?;
    let id = receipt.result["job_id"]
        .as_str()
        .ok_or_else(|| Error::internal("native retry receipt has no task"))?;
    let task = task::read(app, ctx, binding, id, &cancel).await?;
    Ok(
        json!({"success":true,"task":presentation::summary(&task,false,false)?,"operation_id":operation_id,"native":receipt}),
    )
}

fn retry_overrides(
    task: &task::Task,
    values: &serde_json::Map<String, Value>,
) -> Result<input::Create> {
    let changes: input::Create = serde_json::from_value(Value::Object(values.clone()))?;
    if changes.content.is_some() || changes.name.is_some() || !changes.inputs.is_empty() {
        return Err(Error::invalid(
            "retry uses the original frozen UnitIds, not new content",
        ));
    }
    if changes.file_concurrency.is_some()
        || changes.chunk_concurrency.is_some()
        || changes.force
        || changes.clear_manifest
    {
        return Err(Error::new(
            ErrorCode::CapabilityUnavailable,
            "retry does not apply unsupported file-manifest or request-concurrency policy",
        ));
    }
    if let Some(policy)=&changes.dedupe_policy {
        let policy:ImportFileDedupePolicy=serde_json::from_value(json!(policy.trim().to_ascii_lowercase()))?;
        for input in &task.inputs {
            let original=task.files.iter().find(|file|file.input_source_id==input.id).map_or(ImportFileDedupePolicy::None,|file|file.dedupe_policy);
            if policy!=original {return Err(Error::new(ErrorCode::Conflict,"retry retains the original file admission policy; new policy requires file admission"));}
        }
    }
    let wanted = changes.overrides()?;
    for document in &task.catalog.documents {
        let original = &document.effective_options;
        let changed = wanted
            .strategy
            .is_some_and(|value| value != original.strategy)
            || wanted
                .narrative_window_size
                .is_some_and(|value| value.max(200) != original.narrative_window_size)
            || wanted.narrative_overlap.is_some_and(|value| {
                value.min(original.narrative_window_size - 1) != original.narrative_overlap
            })
            || wanted
                .factual_target_size
                .is_some_and(|value| value.max(200) != original.factual_target_size)
            || wanted
                .reference_time
                .as_ref()
                .is_some_and(|value| Some(value) != original.reference_time.as_ref())
            || (values.contains_key("chat_log")
                && changes.chat_log != (original.strategy == ImportTextStrategy::ChatLog));
        if changed {
            return Err(Error::new(
                ErrorCode::Conflict,
                "changed splitting requires a new catalog with parent_catalog_id; retry retains original units",
            ));
        }
    }
    if changes
        .input_mode
        .as_deref()
        .is_some_and(|value| value != "text")
    {
        return Err(Error::invalid(
            "frozen text units cannot change input format on retry",
        ));
    }
    if (changes.scope_type.is_some() || values.contains_key("chat_id"))
        && changes.origin()? != task.request.origin
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "retry retains its parent's actual origin",
        ));
    }
    Ok(changes)
}
