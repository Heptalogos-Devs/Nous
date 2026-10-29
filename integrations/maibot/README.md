# MaiBot 接入 Nous Wave

补丁将 `src/services/memory_service.py` 的后端设为官方 Nous SDK。Planner、Replyer 和 WebUI 继续使用原 `MemoryService`；请求沿 MaiBot 的主事件循环发送。Nous 负责原文接纳、检索、维护任务、人物实体与精确来源。

## 安装

在 MaiBot 源码目录，为当前环境添加本地 SDK wheel：

```sh
uv add "/绝对路径/nous_wave-0.1.0-py3-none-any.whl"
git apply "/绝对路径/Nous-Wave/integrations/maibot/memory-service.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/managed-context.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/diagnostics-unknown.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/bundle-files.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/bundle-unknown.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/import-admin.patch"
```

开发环境也可使用源码包：`uv add --editable "/绝对路径/Nous-Wave/sdk/python"`。SDK 使用说明见 [Python SDK](../../sdk/python/README.md)。

## 配置与启动

由 Nous 操作者建立 Namespace 和 MaiBot Subject，签发该 Subject 的受限凭据。在 MaiBot 实例目录的 `.env` 中填写：

```dotenv
NOUS_URL=http://127.0.0.1:7421
NOUS_NAMESPACE_ID=填写 Namespace UUID
NOUS_SUBJECT_ID=填写 MaiBot Subject UUID
NOUS_TOKEN_FILE=填写受保护凭据文件的绝对路径
NOUS_TRANSFER_ROOT=填写 MaiBot 实例内专用文件传输目录的绝对路径
NOUS_MODEL_CONTEXT_WINDOWS={"实际MaiBot模型配置name":262144}
```

`NOUS_TOKEN_FILE` 指向令牌文件，凭据内容由 SDK 读取。四项连接配置必填；记忆包上传和下载还需设置 `NOUS_TRANSFER_ROOT`。传输目录由 MaiBot 进程管理，例如实例目录下的 `data/nous-transfer`；上传暂存与下载文件均在此目录内。

```sh
uv run --env-file "/MaiBot实例/.env" maibot init --directory "/MaiBot实例"
uv run --env-file "/MaiBot实例/.env" maibot run --directory "/MaiBot实例"
```

已有实例直接使用 `run`。模型与平台连接仍使用 MaiBot 实例配置。本机开发可将实例放在 `Nous-Wave/temp/instances/maibot`，日志、配置和运行数据随实例保存。

## 使用范围

当前接通 `ingest_text`、`ingest_summary`、`search_memory`、`maintain_memory`、`memory_stats` 和人物画像查询；记忆维护在同一事务返回实际认知修订，支持精确地址或检索词冻结的多条 Memory；同一 operation_id 重放保持原选择。模型形成与导入通过真实 Nous 作业等待接口返回结果。人物事实以 `reported` 保存，并保留原文 Memory 的精确修订证据。需要来源时，使用返回的认知引用读取 Nous 来源接口。 `maintain_memory` 支持 reinforce、weaken、protect、unprotect、freeze、forget、restore、trash、purge、decay 与 recycle_bin；受保护记忆的 trash/purge 需要显式 force。

`memory_stats` 与原生 Subject 诊断使用同一授权快照，统计当前认知、任务、向量及模型用量。段落回填按当前语义空间中真实 Memory 输入的请求状态统计。Imported 生产历史单列，并按原生产调用去重；本机消耗保留实际调用用量与货币。

`memory_runtime_admin` 已提供 `get_config`、`list_vector_spaces` 两项只读操作。模型自检、重建和配置写入仍由各正式 owner 续接。没有对应原生资源或队列的字段返回 `null`，原因放在 `unknown` 中。`diagnostics-unknown.patch` 使管理界面保留未知值：空间大小、关系开关和回填状态不会显示成零、关闭或完成。安装后重新构建 MaiBot dashboard。

`integrations.amemorix` 配置控制聊天过滤、检索模式与接纳后的形成任务。向量检索使用 Nous 的模型资源配置；普通文本接纳可直接保存原文。

记忆包入口保留 `export`、`inspect`、`import`、`list`、`resolve_file`、`uninstall` 六个 action。`export` 的 `preview=true` 返回正式导出预览。Bundle 的内容选择、安装范围和资源状态由 Nous 统一处理。WebUI 上传逐块写入传输目录，SDK 将原件流式上传为当前 Subject 所有的 Material，再通过 `material_id` 检查或安装。直接 SDK 使用本地 `path` 时，文件必须位于显式 `transfer_root` 内；已上传的包也可直接传 `material_id`。

导出返回逻辑 `file_name`。下载时 SDK 按 Nous 返回的 Material 流式写入本地受管目录，WebUI 响应结束后清理该下载文件。HTTP 服务与 MaiBot 可部署在不同机器。上传暂存目录由 WebUI 在请求结束时清理，服务端已接纳的 Material 按 Nous 的资源生命周期保存。

`bundle-unknown.patch` 使包预览和安装结果保留未知状态：图片数量、空间兼容性和检索就绪状态按返回值显示；未知项显示“未知”。安装后重新构建 dashboard。

SDK 的 `adapter.managed_files` 提供同一传输目录的 alias 解析和本地扫描。`adapter.upload_import_inputs(staged_files, operation_id=...)` 将 WebUI 暂存文件上传为 Material 并返回原生多文档目录；宿主也可直接使用 `client.import_files` / `client.import_directory` 提交原生导入。

`memory_import_admin` 已接通 settings/guide、宿主路径 aliases/resolve、纯文本上传/粘贴/raw 扫描、任务列表与详情、分块分页、取消和失败单位重试。创建先冻结目录再提交正式 processing；get/list/chunks 的状态、输出和错误来自原生 Task 与不可变 Output。原始文件名来自同一授权读取中的 InputSource；文件路径留在 MaiBot 调用端。

```python
created = await adapter.memory_import_admin(
    action="create_paste", name="notes.txt", content=actual_text,
    llm_enabled=False, dedupe_policy="none",
)
task_id = created["task"]["task_id"]
detail = await adapter.memory_import_admin(action="get", task_id=task_id, include_chunks=True)
```

文件去重支持 `none`、`content_hash`、`manifest`，并保留 `force`、`clear_manifest`。全部文件跳过时仍提交真实 finalization 作业，返回清单回执。JSON/JSONL 使用完整原件与正式记录解析；上传未指定格式时由 `.json`、`.jsonl` 扩展名选择格式；显式 input_mode 保持调用方选择。请求级并发由 Nous 共享队列配置管理。
重试使用同一目录的实际失败 UnitId；相同切分参数可重复传入，改变切分必须先创建带 parent_catalog_id 的新目录。聊天绑定作为真实 Import 事件 origin 保存，不增加读取权限。语义角色、逐字 quote 和向量状态与原生处理一致。chunk 输出来自实际 UnitAttempt；可选向量失败保留 vector_error 与已采纳的 Output。时间提取是形成调用的显式选项。当前时间归一化只接受明确 offset 的 RFC3339，日期粒度与未知时区不会在调用端补值。

JSON 记录支持 `records`、`paragraphs`、`docs` 包络与 JSONL 原顺序，错误定位保留到具体记录。LPMM 转换、时序回填和 MaiBot 数据库迁移保留各自处理合同。

`memory_source_admin` 支持 list、delete 和 batch_delete，按完整来源标签进入原生来源管理。list 返回实际授权范围内的段落数量和更新时间；手动删除保持此管理接口覆盖保护的原有语义。回执中的删除 operation_id 为本实例真实 trash 提交身份，后续恢复使用同一个值：

```python
plan = await adapter.memory_delete_admin(
    action="preview", mode="source", selector={"sources": [source_label]},
)
removed = await adapter.memory_source_admin(
    action="delete", source=source_label, operation_id=delete_request_id,
)
restored = await adapter.memory_delete_admin(
    action="restore", mode="source", operation_id=removed["operation_id"],
    request_id=restore_request_id,
)
operation = await adapter.memory_delete_admin(
    action="get_operation", operation_id=removed["operation_id"],
)
```

source `preview` 与 `execute(dry_run=true)` 由同一来源事务生成计划后回滚，显示目标原修订、预计退役对象和向量关联数；执行时重新核验。`get_operation` 与 `list_operations(mode="source")` 读取同一快照中的逻辑删除记录，已恢复记录显示 restored，并附真实恢复回执。删除计数来自实际 Memory、退役 Entity／Association 和向量关联行数；图谱修订另列在 native_result。

尚未提供的管理动作返回 `CapabilityUnavailable`。画像与统计异常向调用者传播，写入、检索及其他管理请求沿原有明确失败结果呈现。


## 模型调用与托管上下文

`managed-context.patch` 在 MaiBot 的正式 `ResponseRequest` 与四类模型客户端边界接线。设置 `NOUS_MODEL_CONTEXT_WINDOWS` 后，有真实聊天会话的模型调用会先通过 SDK 提交持久准备任务，再把授权上下文渲染为实际 ContextItem。OpenAI Chat、Responses、Gemini 和插件客户端在构造完整 provider 输入后、发送前测量并接纳；结果返回后报告 completed、failed 或 cancelled。模型接触与实际使用分别记录，宿主不会把成功回复全部算作实际使用。

窗口映射使用 MaiBot `model_config.toml` 的模型 **name**，必须填写供应商确认的窗口值。例中 `262144` 适用于已核实的 Doubao Seed 2.0 mini 256k 窗口（[供应商模型表](https://www.volcengine.com/docs/82379/1330310)）；其他模型使用各自窗口。测量采用实际完整 provider 输入的 UTF-8 字节上界，包含工具、格式约束与多模态数据；输出预留来自该次真正发送的参数。图片输入还要求 `NOUS_MODEL_IMAGE_TOKEN_BOUNDS={"模型name":每张图片的供应商token上界}`，避免用文本字节估计视觉 token。

会话身份来自 MaiBot 正式 chat_manager 创建或读取的 session_id。来源权限按当前 conversation_ref 限定；没有会话的后台模型调用不派生聊天身份。history_generation 是实际输入历史的内容摘要，invocation_id 标识真实宿主模型调用。

Nous 的 `contexts` 配置为每个 consumer 管理预算、居民与可选 Steward。`consumers.maibot.steward_role="steward"` 使用同一模型资源图、Prompt 和调用账本；留空时只使用确定性的检索与预算编译。Steward 只选择当前可见条目，综合结果带 `steward_interpretation` 与归因。居民过期使用 DBOS 正式 schedule，接触不会刷新其实际使用时间。


图谱搜索通过现有 MemoryService.graph_admin → SDK invoke → `memory_graph_admin` 接入：

```python
from nous_wave.contracts import CognitiveRef

result = await adapter.memory_graph_admin(action="search", query="项目", limit=50)
# MaiBot 的既有 RPC 入口传递相同合同。
result = await adapter.invoke("memory_graph_admin", {"action": "search", "query": "项目", "limit": 50})
for item in result["items"]:
    source = await client.source(subject_id, CognitiveRef.model_validate(item["reference"]))
```

search 按实体名称、真实身份、关系两端名称与谓词进行精确、前缀、子串匹配。items 保留旧界面的 type/title/matched_field/matched_value、entity_hash 或 relation_hash；这些身份指向实际 Native 认知，并附完整 reference 与来源根。置信度只有真实记录存在时返回。图谱写入动作仍要求相应领域操作，当前不会以修改投影替代认知变更。

CLI 对应 `nous graph-search <namespace-id> <subject-id> <query> --limit 50`；MCP 的 `memory_graph_admin` 接受 search action、query 和 limit，沿同一来源权限读取。
