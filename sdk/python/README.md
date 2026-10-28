# Nous Wave Python SDK

异步客户端使用正式 Python 领域合同，供 Agent、MaiBot 和普通 Python 程序调用。安装：

```sh
uv add --editable /绝对路径/Nous-Wave/sdk/python
```

```python
from nous_wave import ClientOptions, NousClient
from nous_wave.contracts import AdmitFiles, ImportDocument, QueryRequest

async with NousClient(
    service_url,
    credential_file=protected_credential_file,
    options=ClientOptions(language="zh-CN"),
) as client:
    namespace = await client.create_namespace("研究")
    subject = await client.create_subject(namespace.id, name="助手")
    imported = await client.admit_documents(namespace.id, subject.id, AdmitFiles(
        inputs=(ImportDocument(text=actual_text, filename="notes.md"),),
        llm_enabled=False, vectors="disabled",
    ))
    result = await client.query_cognition(namespace.id, subject.id,
        QueryRequest(text="当前任务", mode="lexical"))
```

返回值为 Pydantic 合同或明确的 JSON 对象。`nous_wave.contracts` 可以独立导入，不加载 HTTP 客户端。`NousError` 保留服务端错误码、消息与上下文。写入重试复用同一 `operation_id`；网络错误保留该身份供继续。客户端连接、凭据读取、Trace、语言、超时、代理和下载校验共用一个 transport。

`ClientOptions.cognitive_clock=TimePoint(...)` 显式提供认知形成时钟；未设置时服务使用物理 UTC。Context 的 `usage.world_time` 仅描述用途中的世界坐标。作业调度、调用预算与实际用量使用物理时间。

## 来源与地址

```python
from nous_wave import SourceReadScope
from nous_wave.source_scope import CognitionSource, MediaSource, OriginalMedia

whole = client.with_source_read(SourceReadScope(
    subject_id=subject_id,
    source_refs=(MediaSource(reference=OriginalMedia(material_id=material_id)),),
))
single = whole.with_source_read(SourceReadScope(
    subject_id=subject_id,
    source_refs=(CognitionSource(reference=exact_cognitive_ref),),
))
```

范围可选择完整原件、精确区域、单个认知修订、Observation、工作上下文锚点或实际 conversation origin。认知与媒体 selector 均接受对应类型的 Proquint 地址，由服务端统一 ReferenceDirectory 解析。多个范围与凭据授权按语义交集收窄；空范围返回空读取权限。单认知范围保持精确引用，读取容器原件仍需原件授权。

`resolve_reference(subject_id,address,as_of_commit=...)` 返回 `ReferenceResolution`。对象短地址在指定知识截点选择修订，固定地址保留精确修订。`source(subject_id,reference)` 读取认知或媒体来源；`memory_source(memory_id,revision=...)` 读取记忆修订的真实来源。`cognitive_revision(namespace_id,subject_id,reference)` 返回准确修订。

## 文件与导入

`upload_file`、`upload_stream` 流式接纳原件；上传必须指定 `owner_subject_id`。`expected_content_hash` 可核验实际上传字节。`download_material` 和 `download_region` 使用相同 scope 元数据、长度与散列验证，失败会清理未完成文件。

```python
from nous_wave.contracts import FreezeCatalog, ProcessCatalog

material = (await client.upload_file(namespace_id, local_file,
    owner_subject_id=subject_id)).result
catalog = await client.freeze_catalog(namespace_id, subject_id, FreezeCatalog(
    inputs=(ImportDocument(material_id=material.id, format="json", filename="records.json"),),
    strategy_override="factual",
))
submitted = await client.submit_catalog(namespace_id, subject_id,
    ProcessCatalog(catalog_id=catalog.id, llm_enabled=True, vectors="metadata_allowed"))
job = (await client.wait_success((submitted.job_id,),
    namespace_id=namespace_id, subject_id=subject_id))[0]
```

JSON 接受 `records`、`paragraphs`、`docs` 包络，JSONL 保留记录顺序。每条记录可带 `external_id,text,source,source_type,event_time,observed_at,origin,tags,metadata,structured`，服务端保留原件与精确区域。拆分支持 auto、factual、narrative、quote、chat_log；quote 逐字保存并跳过形成模型。时间语境通过显式 timezone、reference_time 与 chat_reference_time 传给导入 owner。

`admit_documents(...,AdmitFiles(...))` 完成文件资格去重、冻结与处理，返回 `ImportResult`。`dedupe_policy` 为 none、content_hash 或 manifest；force 重新处理，clear_manifest 清除当前资格并保留历史事实。全部跳过时 `catalog=None`，跳过身份在 `skipped_success_ids` 中。异步管理创建即使全部跳过也返回真实 finalization 作业。

`import_files` / `import_directory` 先上传准确原件，再调用同一接纳用例。文件可为路径，或 `{staged_path,filename,size}` / `{path,name,format,source,file_identity}`。SDK 不解析正文。本地扫描与文件路径限制在显式的 `transfer_root`；`ManagedFiles` 提供 raw/lpmm/maibot/converted aliases，拒绝路径或符号链接越界。原路径留在宿主。

`import_catalog`、`import_unit_output` 返回冻结 Unit 与不可变 Output。单位尝试保留实际 job_id、阶段、失败与可选 vector_error；未执行单位保持 pending。`vectors="required"` 要求向量完成，`metadata_allowed` 保留成功的正文 Output 及独立向量失败，`disabled` 不请求向量。

`job`、`jobs`、`wait_jobs`、`wait_success`、`cancel_job` 使用正式 DBOS 状态。等待采用有上限的退避和调用超时。失败作业保留已提交的业务事实。

## 查询与管理

`query` / `query_cognition` 支持 lexical、dense、hybrid、vcp，认知与媒体 domains、混合 parts、精确引用、事件时间、来源过滤及历史截点。`query_manual()` 返回当前原生 NousQL 手册。

`adopt_cognition`、`merge_concepts`、`split_concept`、`resegment_episodes` 使用共享合同；`update_memory` 接受正式 `MemoryChange` 生命周期命令。`adopt_seed` 保存显式配置文档及 SelfFacet 证据，`self_profile` 读取当前正式 Self 画像。

来源管理使用 `source_directory`、`preview_sources`、`trash_sources`、`restore_sources`、`purge_sources` 及共享 `SourceCommand`；恢复保留实际 Observation 身份。Bundle 使用 `export_bundle` 与 `install_bundle` 合同，A-Memorix 提供 ZIP 文件操作。导出产物保留实际依赖闭包，读取与来源 purge 由材料 owner 共同约束。

`configuration`、`prompts`、`model_contract`、`model_preview`、`model_call`、`backup` 接正式管理入口。`subject_diagnostics` 返回授权范围内的认知、向量、任务与实际 token 用量。不同凭据的 read/write/manage 能力由服务核验。

## 每次模型调用的 Context

```python
from nous_wave.contracts.context import ContextUsage

binding = client.managed(
    namespace_id=namespace_id, subject_id=subject_id,
    session_id=actual_session_id, consumer_id="reply", instance_id=actual_worker_id,
    source_read=trusted_source_scope,
)
prepared = await binding.prepare(
    usage=ContextUsage(purpose="reply", intent=actual_intent),
    max_items=64, max_text_bytes=65536,
    current_sources=actually_required_sources,
)
applied = binding.apply(clean_host_input, prepared, renderer, serialize_input)
await binding.admit(applied, invocation_id=actual_invocation_id,
    history_generation=actual_history_generation,
    measurement=host_measure_complete_input(applied.input))
# 在此调用真实模型。
await binding.report(applied, invocation_id=actual_invocation_id,
    outcome=actual_outcome, occurred_at=actual_completion_time,
    uses=actually_observed_uses)
await binding.close()
```

prepare 提交真实持久任务并等待。renderer 接收独立副本并返回完整输入；serialize_input 编码该实际输入。`HostInputMeasurement` 包含完整输入 tokens、真实输出预留、窗口与测量来源，超窗会拒绝。SDK 管理 RESET/APPEND 游标，应用后输入变动会使接纳失败。没有显式来源范围时 managed 采用空读取范围。

`open_context_session` / `context_session` / `close_context_session` 管理真实宿主会话。`change_context_residents` 使用 `ChangeResidents.expected_revision` 修改居民；`change_work_context` 支持创建、编辑、暂停、恢复、结束、pin/unpin 与清理。工作指令和显式 current_sources 是必需项，预算不足会报错。

`contexts` 配置限制 consumer 预算与来源类型。配置 `steward_role` 后使用统一模型资源、Prompt、预算与账本；Steward 只选择当前授权候选，综合保持独立归因。居民使用 DBOS schedule 维护，模型接触、实际使用和完成结果分别记录。

A-Memorix `AMemorixAdapter` 保留六项工具、来源、导入、包与运行时管理入口。完整 MaiBot 补丁、模型窗口配置与真实发送边界见 [MaiBot 集成](../../integrations/maibot/README.md)。
