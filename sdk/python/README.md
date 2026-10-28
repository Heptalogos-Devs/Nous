# Nous Wave Python SDK

异步客户端面向 Agent、MaiBot 集成和普通 Python 程序。HTTP 连接、代理、凭据、超时、Trace 和错误解析由一个 transport 管理；模型调用与持久作业继续由 Nous 服务执行。

```powershell
. ./scripts/dev-env.ps1
uv pip install --python <虚拟环境中的Python> -e sdk/python
```

使用已有实例与受保护的凭据文件：

```python
from nous_wave import ClientOptions, NousClient

async with NousClient(
    service_url,
    credential_file=credential_file,
    options=ClientOptions(language="zh-CN"),
) as client:
    identity = await client.identity({"action": "whoami"})
    result = await client.query(subject_id, "本轮开发尚未完成的工作")
```

`configuration`、`identity`、`prompts`、`media`、`cognition`、`contexts`、`bundles` 接收与 HTTP/MCP 相同的正式操作对象。常用材料、记忆、查询、向量、任务和备份有具名方法。`request` 用于直接调用其他公开操作。写入方法接受 `operation_id`；重试同一个实际动作时保留此值，网络异常的 `context.operation_id` 可供继续。

`upload_file` 与 `download_material` 流式传输原件。`query(..., media=[...])` 接受精确 MediaReference，文字与媒体一起构成联合查询。`with_source_read(scope)` 创建共享连接池的受限客户端，所有操作沿相同请求元数据追加范围；调用参数继续在此范围内收窄。

绑定本次提供的整份导入原件，继续读取其正式解析记录和执行模型形成：

```python
selected = client.with_source_read({
    "subject_id": subject_id,
    "source_refs": [{
        "kind": "media",
        "reference": {"kind": "original", "material_id": input_material_id},
        "observation_id": None,
    }],
    "origin_conversation_refs": [],
})
```

整份原件的范围覆盖正式绑定的记录及其 Import 事件；单条记忆的范围保留自身的来源边界，容器原件正文另行授权。多个范围按交集约束，空 `source_refs` 与空会话来源表示空读取范围。

`import_text(request, operation_id=...)` 返回持久任务回执，`receipt["result"]["job_id"]` 指定导入任务。`wait_success([job_id])` 返回处理完成的 JobRecord；其 `result.items` 按原记录顺序保存接纳、重放或拒绝结果。成功项的 `admission` 包含准确材料、Import 事件、Memory 和可选形成任务身份。`job(job_id)` 返回 `job`、`queue`、`model_calls` 详情，`cancel_job(job_id)` 取消尚未完成的工作。已提交的记录进度在重启后继续保留。

请求的 `completion` 默认为 `"accepted"`，Job 在记录接纳处理完毕后完成。需要形成完成后再继续时，设置 `form_memory=True, completion="formed"`，再等待这一个导入 Job；其持久依赖保存实际形成任务身份，重启继续等待。形成任务失败会使父 Job 失败并保留已接纳资料与依赖信息。

`preview_text_import(namespace_id, subject_id, document, overrides=...)` 只读预览纯文本拆分：

```python
preview = await client.preview_text_import(
    namespace_id, subject_id,
    {"format": "text", "input": {"kind": "inline", "content": actual_text}},
    overrides={"strategy": "narrative", "narrative_window_size": 1600, "narrative_overlap": 400},
)
```

已有原件可用 `input={"kind":"material","material_id":material_id}`。结果包含原文 hash/字节数、effective_options、selected_strategy、units 与 warnings；每个 unit 的 start_byte/end_byte 是原文 UTF-8 字节范围，text 与该范围原文一致。预览使用 `imports.text` 的有效默认值与本次 overrides；支持 auto/narrative/factual/quote/chat_log，不创建材料或作业、不调用模型。

冻结一次实际目录，再读取或选择原 UnitId 供重试使用：

```python
frozen = await client.freeze_import_catalog({
    "namespace_id": namespace_id,
    "subject_id": subject_id,
    "inputs": [{"format": "text", "input": {"kind": "material", "material_id": input_material_id}}],
    "overrides": {"strategy": "narrative"},
}, operation_id=catalog_operation_id)
catalog_id = frozen["result"]["catalog_id"]
catalog = await client.import_catalog(namespace_id, subject_id, catalog_id)
inputs = await client.retry_import_inputs(
    namespace_id, subject_id, catalog_id, frozen["result"]["unit_ids"][:1],
)
```

这些读取沿正文和标题 context 的精确来源权限返回原单位文本及 source_refs，保留冻结配置与 UnitId。重新配置拆分时，在新的 freeze 请求传入 parent_catalog_id；普通重试继续使用原目录。冻结回执给出来源目录身份，实际处理由 `process_import_units` 提交。

```python
processing = await client.process_import_units({
    "namespace_id": namespace_id, "subject_id": subject_id,
    "catalog_id": catalog_id, "unit_ids": None,
    "llm_enabled": True, "extract_time": True, "vectors": "required",
}, operation_id=processing_operation_id)
await client.wait_success([processing["result"]["job_id"]])
parent = await client.job(processing["result"]["job_id"])
for item in parent["job"]["result"]["unit_jobs"]:
    child = await client.job(item["job_id"])
    if child["job"]["result"] is not None:
        output = await client.import_unit_output(
            namespace_id, subject_id, child["job"]["result"]["output_id"],
        )
```

处理通过 `import_narrative`、`import_factual` 和独立 `import_time` 角色调用真实模型；需要时配置对应角色。quote 和关闭 LLM 的路径保存逐字内容，不产生模型调用。结果中的 Memory/认知引用、semantic/temporal call 和错误来自不可变 output 事实；parent 只引用真实 unit Jobs。`vectors=disabled` 不建立索引作业，`metadata_allowed` 保留实际索引请求及准备错误，`required` 等待真实索引依赖成功。已有父任务可用 `wait_jobs` 等待；取消只影响其独占子任务，共享索引工作继续由索引 owner 管理。

Subject 提供自己的文件时，传入 `upload_file(namespace_id, path, owner_subject_id=subject_id)`。已有异步字节流使用 `upload_stream(namespace_id, chunks, byte_len=length, owner_subject_id=subject_id)`；两者使用同一传输与接纳用例。原件归该 Subject 读取，接触或导入来源事件由后续显式操作记录。CLI 对应 `material put --namespace <Namespace UUID> --owner-subject <Subject UUID> --file <本机文件>`。

需要文件成功记录去重及自动冻结、处理时，使用 `admit_import_files(request, operation_id=...)`。本机文件可先通过公共文件边界上传并生成调用端身份：

```python
from nous_wave.imports import upload_import_file_documents

inputs = await upload_import_file_documents(
    client, namespace_id, subject_id, ["source/raw/notes.md"],
    transfer_root=instance_transfer_directory, alias="raw",
    operation_id=import_operation_id,
)
receipt = await client.admit_import_files({
    "namespace_id": namespace_id, "subject_id": subject_id, "inputs": inputs,
    "overrides": {"strategy": "narrative"},
    "llm_enabled": True, "extract_time": False, "vectors": "required",
    "origin": None, "dedupe_policy": "manifest", "force": False, "clear_manifest": False,
}, operation_id=import_operation_id)
job_id = receipt["result"]["processing_job_id"]
if job_id is not None:
    await client.wait_success([job_id])
```

每项输入返回 process 或带实际 success_id 的 skip。全部 skip 时 catalog_id 与 processing_job_id 为 None；其余输入按实际冻结目录提交处理。dedupe_policy 可为 none、content_hash（默认）或 manifest。file_identity 可显式提供；alias 文件身份由调用端生成，保留 alias 与相对路径，绝对主机路径留在本机。缺少身份时 manifest 按内容 hash；force 显式重新处理，clear_manifest 清除当前 namespace／subject／typed origin 下全部当前资格，历史成功记录保留。已上传的输入直接使用 `{document:{format,input:{kind:"material",material_id}},file_identity}`。

`import_files(namespace_id, subject_id, files, transfer_root=...)` 将本机文件逐份流式上传，再以 Material 输入目录提交一个正式导入任务。`files` 接受路径列表，或 WebUI 的 `{staged_path, filename, size}` 列表；也可用 `{path, name, format, source}` 明确格式与语义来源。格式按 txt/md、json、jsonl 扩展名选择，`input_format` 可显式指定。SDK 不解析或改写文件内容。

```python
receipt = await client.import_directory(
    namespace_id,
    subject_id,
    transfer_root=instance_transfer_directory,
    alias="raw",
    relative_path="notes",
    pattern="*.md",
    recursive=True,
    form_memory=True,
    completion="formed",
    operation_id=import_operation_id,
)
job = (await client.wait_success([receipt["result"]["job_id"]]))[0]
```

`transfer_root` 必须是宿主管理的绝对目录。`ManagedFiles(transfer_root).path_aliases()` 返回该目录内的 `raw=source/raw`、`lpmm=source/lpmm`、`maibot=source/maibot`、`converted=converted`；`resolve_path(alias, relative_path)` 读取本机文件状态。alias 路径拒绝绝对路径、盘符、上级目录与符号链接越界。目录扫描选择 txt/md/json/jsonl，原路径留在调用端。

这些便利方法保留调用者原文件，WebUI 自行清理请求的上传暂存目录。上传已接纳的 Material 继续由 Nous 管理，重试从正式原件恢复。重复同一导入时保留 `operation_id` 和文件顺序，SDK 为各文件上传派生稳定操作身份。`import_files` 和 `import_directory` 均把 `form_memory` 与 `completion` 传给同一原生导入操作，返回持久回执后可用 `wait_success` 等待所选完成模式。

`source(subject_id, reference, observation_id=...)` 读取精确认知或媒体的来源。`memory_source(memory_id, revision=...)` 读取指定历史修订；省略修订时读取当前记忆。媒体的 `observation_id` 指定本次实际接触或导入，认知自身保留形成时的来源事件。

`source_directory(namespace_id, subject_id)` 列出授权范围内的精确来源标签及实际记忆、材料、事件和受保护对象数量。使用目录中的 source 值整批移入回收站，并保存回执以恢复同一批次：

```python
directory = await client.source_directory(namespace_id, subject_id)
plan = await client.preview_sources(namespace_id, subject_id, [source_label])
trashed = await client.trash_sources(
    namespace_id, subject_id, [source_label],
    reason="撤下这组资料", operation_id=trash_operation_id,
)
restored = await client.restore_sources(
    namespace_id, subject_id, trashed["commit_seq"],
    reason="恢复原批次", operation_id=restore_operation_id,
)
```

含受保护记忆的选择需要显式 `force=True`。preview 返回目标原修订、图谱计划及向量关联退役数量；预览事务回滚，执行时重新核验。恢复核对原批次的当前修订并保留原 Frozen、固定和保护期限；后续修改会触发 Conflict 并保留当前状态。通用 `sources(request)` 接受同一 HTTP/MCP 的正式操作对象；全部方法共享当前连接、凭据和 SourceReadScope。

`source_operation(namespace_id, subject_id, commit_seq)` 与 `source_operations(..., limit=50, before_commit_seq=...)` 读取真实 source 事件。需要某次删除的逻辑状态时，用 `sources({"action":"get_trash_operation","request":{"namespace_id":namespace_id,"subject_id":subject_id,"commit_seq":trash_commit_seq}})`；`list_trash_operations` 使用相同 scope、limit 和 before_commit_seq 请求逻辑删除分页。每项的 trash 保留原回执，restore 保存实际恢复回执。

A-Memorix `memory_source_admin` 的 list／delete／batch_delete 使用同一来源 owner。手动删除按该接口原有语义显式启用保护覆盖，原生 `trash_sources` 仍默认保留保护检查。结果的 operation_id 和 delete_operation_id 是本实例真实 trash commit_seq 字符串；原生幂等身份保存在 native_operation。`memory_delete_admin` 的 source preview 与 execute(dry_run=true) 返回未提交计划；execute、精确 restore、get_operation 与 list_operations 返回相同真实批次，恢复后逻辑状态为 restored。

`source` 同时接受规范引用对象和结果中的词序引用。`resolve_reference(subject_id, address, as_of_commit=...)` 将对象地址绑定到所选知识截点，固定修订地址保持原修订；结果包含规范身份及本机地址。Namespace 操作者可通过 `reconcile_references` 为已有资料补齐目录。知识包恢复后，原地址沿安装来源映射解析。

`subject_diagnostics(subject_id)` 读取当前授权范围内的认知、任务、向量和实际用量统计。本机消费与 Imported 生产历史分别汇总，金额保留货币；缺失量为 `None` 并附原因。A-Memorix adapter 的 `memory_stats()`、`memory_runtime_admin(action="get_config")` 和 `memory_runtime_admin(action="list_vector_spaces")` 使用同一诊断用例；MaiBot 接入和未知值展示补丁见 [集成说明](../../integrations/maibot/README.md)。

A-Memorix adapter 的 `memory_import_admin(action=...)` 将纯文本创建接到冻结目录与正式处理任务，`create_upload` 接受受管 `staged_files`，`create_raw_scan` 扫描本机 `raw` alias。路径不发往服务器。当前创建显式传 `dedupe_policy="none"`；文件清单去重和请求级并发尚未接入，传入时返回能力错误。`get/list/get_chunks/cancel/retry_failed` 使用原生任务和稳定 UnitId，具体使用见 MaiBot 集成说明。

`process_import_units` 的请求可附 `origin`（实际 ObservationOrigin）和 `retry_parent_job_id`。CLI 对应 `--conversation-ref` / `--external-object-ref` 与 `--retry-parent-job`。重试原任务引用只记录尝试来源，不构成独占 parent_job_id 所有权；原生 owner 校验同作用域、同目录和实际失败单位。

## 每次模型调用的上下文

会话使用 `contexts` 的 `open_session` 操作建立。宿主保存自己的真实历史、工具状态、会话 ID 和历史代次；每个独立执行者使用自己的 `consumer_id` / `instance_id`。

```python
binding = client.managed(
    subject_id=subject_id,
    session_id=session_id,
    consumer_id=consumer_id,
    instance_id=worker_id,
    source_read=trusted_source_scope,
)

prepared = await binding.prepare(
    usage=actual_usage,
    max_items=contribution_item_limit,
    max_text_bytes=contribution_byte_budget,
    current_sources=actually_supplied_sources,
    query=optional_query,
)
applied = binding.apply(clean_host_input, prepared, renderer, serialize_input)
measurement = host_measure_complete_input(applied.input)
await binding.admit(
    applied,
    invocation_id=invocation_id,
    history_generation=history_generation,
    measurement=measurement,
)
```

`renderer(base, segments, usage, epistemic_view)` 返回完整模型输入，输入参数已独立复制。`serialize_input` 返回该输入的实际序列化字节。宿主用自己的测量器生成 `HostInputMeasurement`，包含完整输入 token、预留输出、窗口上限与测量来源；SDK 和服务都会拒绝超过窗口的接纳。

模型调用结束后，用实际结果和时间报告：

```python
await binding.report(
    applied,
    invocation_id=invocation_id,
    outcome=actual_outcome,
    occurred_at=actual_completion_time,
    uses=actual_meaningful_uses,
)
await binding.close()
```

`apply` 管理自身的 RESET/APPEND 片段与游标；工具续轮、模型切换和压缩后继续对净化后的宿主输入执行同一流程。`admit` 记录实际呈现，`report` 记录实际结果及明确使用事件。宿主持久保存接纳回执、调用 ID、完成时间与使用事件；进程重启后可用 `client.report_context(admission_id, invocation_id=..., outcome=..., occurred_at=..., uses=...)` 继续提交同一反馈。

读取范围来自可信宿主接纳的来源或会话 origin。普通查询继承已认证权限；managed binding 没有显式范围时由服务采用空读取范围。精确引用和派生来源保持独立身份，资料文本不会成为对 SDK 或宿主的执行指令。
