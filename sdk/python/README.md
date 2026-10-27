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

`source(subject_id, reference, observation_id=...)` 读取精确认知或媒体的来源。`memory_source(memory_id, revision=...)` 读取指定历史修订；省略修订时读取当前记忆。媒体的 `observation_id` 指定本次实际接触或导入，认知自身保留形成时的来源事件。

`source` 同时接受规范引用对象和结果中的词序引用。`resolve_reference(subject_id, address, as_of_commit=...)` 将对象地址绑定到所选知识截点，固定修订地址保持原修订；结果包含规范身份及本机地址。Namespace 操作者可通过 `reconcile_references` 为已有资料补齐目录。知识包恢复后，原地址沿安装来源映射解析。

`subject_diagnostics(subject_id)` 读取当前授权范围内的认知、任务、向量和实际用量统计。本机消费与 Imported 生产历史分别汇总，金额保留货币；缺失量为 `None` 并附原因。A-Memorix adapter 的 `memory_stats()`、`memory_runtime_admin(action="get_config")` 和 `memory_runtime_admin(action="list_vector_spaces")` 使用同一诊断用例；MaiBot 接入和未知值展示补丁见 [集成说明](../../integrations/maibot/README.md)。

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
