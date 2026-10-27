# MaiBot 接入 Nous Wave

补丁将 `src/services/memory_service.py` 的后端设为官方 Nous SDK。Planner、Replyer 和 WebUI 继续使用原 `MemoryService`；请求沿 MaiBot 的主事件循环发送。Nous 负责原文接纳、检索、维护任务、人物实体与精确来源。

## 安装

在 MaiBot 源码目录，为当前环境添加本地 SDK wheel：

```sh
uv add "/绝对路径/nous_wave-0.1.0-py3-none-any.whl"
git apply "/绝对路径/Nous-Wave/integrations/maibot/memory-service.patch"
git apply "/绝对路径/Nous-Wave/integrations/maibot/diagnostics-unknown.patch"
```

开发环境也可使用源码包：`uv add --editable "/绝对路径/Nous-Wave/sdk/python"`。SDK 使用说明见 [Python SDK](../../sdk/python/README.md)。

## 配置与启动

由 Nous 操作者建立 Namespace 和 MaiBot Subject，签发该 Subject 的受限凭据。在 MaiBot 实例目录的 `.env` 中填写：

```dotenv
NOUS_URL=http://127.0.0.1:7421
NOUS_NAMESPACE_ID=填写 Namespace UUID
NOUS_SUBJECT_ID=填写 MaiBot Subject UUID
NOUS_TOKEN_FILE=填写受保护凭据文件的绝对路径
```

`NOUS_TOKEN_FILE` 指向令牌文件，凭据内容由 SDK 读取。四项配置均为必填。

```sh
uv run --env-file "/MaiBot实例/.env" maibot init --directory "/MaiBot实例"
uv run --env-file "/MaiBot实例/.env" maibot run --directory "/MaiBot实例"
```

已有实例直接使用 `run`。模型与平台连接仍使用 MaiBot 实例配置。本机开发可将实例放在 `Nous-Wave/temp/instances/maibot`，日志、配置和运行数据随实例保存。

## 使用范围

当前接通 `ingest_text`、`ingest_summary`、`search_memory`、`maintain_memory`、`memory_stats` 和人物画像查询；维护任务通过 Nous 作业等待接口返回实际结果。人物事实以 `reported` 保存，并保留原文 Memory 的精确修订证据。需要来源时，使用返回的认知引用读取 Nous 来源接口。

`memory_stats` 与原生 Subject 诊断使用同一授权快照，统计当前认知、任务、向量及模型用量。段落回填按当前语义空间中真实 Memory 输入的请求状态统计。Imported 生产历史单列，并按原生产调用去重；本机消耗保留实际调用用量与货币。

`memory_runtime_admin` 已提供 `get_config`、`list_vector_spaces` 两项只读操作。模型自检、重建和配置写入仍由各正式 owner 续接。没有对应原生资源或队列的字段返回 `null`，原因放在 `unknown` 中。`diagnostics-unknown.patch` 使管理界面保留未知值：空间大小、关系开关和回填状态不会显示成零、关闭或完成。安装后重新构建 MaiBot dashboard。

`integrations.amemorix` 配置控制聊天过滤、检索模式与接纳后的形成任务。向量检索使用 Nous 的模型资源配置；普通文本接纳可直接保存原文。

尚未提供的管理动作返回 `CapabilityUnavailable`。画像与统计异常向调用者传播，写入、检索及其他管理请求沿原有明确失败结果呈现。
