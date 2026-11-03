# Nous 模型、媒体与内容架构

## 当前产品目标

在已有运行基础上，交付能真正处理资料的工具能力：接纳内容、准备模型输入、执行语言与媒体任务、保存结果、展开来源并继续读取。此时的内容记录具有材料处理语义；Note、Journal、Episode、Ego 等认知对象的形成规则后续接入。

保持 CPython 3.14、uv、SQLAlchemy Core/SQLite、asyncio、Pydantic 配置、logging 与 OpenTelemetry。已有独立服务可按合同复用；新能力不通过恢复 legacy 整个运行栈接入。

## 所有权与调用方向

```text
CLI（nous tools / config / runtime）
        ↓
ConfigurationOperations / GenerationOperations / MediaAnalysis / EmbeddingOperations
        ↓                   ↓                   ↓
ArtifactStore / ResultStore   PromptCatalog + PromptCompiler   MediaPreparer
        ↓                   ↓                   ↓
ContentStore + Database      ModelExecutor / EmbeddingExecutor  Pillow / PDFium / FFmpeg
                            ↓
                      ModelConnections → Network + Credentials → 成熟 SDK → 统一网关

Bootstrap 显式构造资源；Execution 提供操作期限、取消和关闭。
logging / OpenTelemetry API 提供基础埋点，不反向依赖以上业务。
```

表中名称表示完整责任；不要求每个名字单独成为类、文件或发行包。语言任务模板归任务 owner，协议映射归依赖，语义采用归后续领域。

| 信息或行为 | 负责者 |
|---|---|
| 配置定义、解析、有效快照、文件保存 | Configuration |
| 一次配置变更的实例/时间/向量绑定及采用 | 应用层 ConfigurationOperations，调用实际 owner |
| 不可变字节、流式读写、物理去重 | ContentStore |
| 输入资料与派生文件的可读身份 | ArtifactStore |
| 一次已成功采用处理的结果、实际输入、生成出处 | ResultStore；失败与部分完成由调用用例返回 |
| 图片/PDF/音视频解码、选区与转换 | MediaPreparer 及模态内聚实现 |
| 三种媒体解释策略和阶段衔接 | MediaAnalysis |
| 模板选择、角色顺序和确定渲染 | PromptCatalog / PromptCompiler |
| 确定模型请求、网络尝试和响应 | ModelExecutor；不访问业务数据库 |
| 向量空间及实际向量 | Embedding owner |

## 正式数据保持少而明确

Artifact 是一份不可变文件或文字内容的产品引用。ProcessingResult 记录一次成功处理的输入、输出、参数与 producer，可以由确定性转换或模型调用产生。CAS 摘要仅表示字节；同样字节的两次接纳可拥有不同来源记录。

完整操作由应用层开始，一个短事务提交所需结果和内容引用。模型请求、媒体解码和字节发布在事务外完成。结果采用不经过事件总线层层触发；后续索引和认知工作按实际需要增加。

## 入口与操作

`nous tools` 是长期保留的产品入口。CLI 只解析输入、调用完整操作和呈现。未来 HTTP/MCP/SDK 复用同一应用操作，不要求把内部方法一一公开为工具。

`model run`、`media analyze`、`embedding run` 可以自动接纳本地输入并返回正式结果。用户无需手动执行 prepare、publish、attach、编译、模型提交等内部步骤。高级用户可先接纳资料再按短引用继续操作。

## 同轮修正

配置修改的路径、时间和 Embedding 绑定检查从 CLI 和 bootstrap 内联回调集中到应用用例。构造依赖后形成可用对象，不向已经发布的 Configuration 回填服务。

基础错误、纯标识和观测 API 放到独立的下层模块。Storage/Execution 不因调用 `get_tracer()` 而导入含 Configuration、OTel SDK 和 exporter 的整个观测装配。正式日志和 SDK 生命周期仍归 bootstrap。

时间域的纯语义校验共用一个定义，尤其 opaque 域没有 tick_seconds。schema/表由具体 owner 提供，组合根在显式初始化时收集；数据库模块不逐个导入所有认知和媒体服务。

## 供应商中立

生产逻辑根据明确 protocol、能力与任务配置工作。New API 的 Docker、后台管理端点和令牌获取只属于 [开发接入](development/local-gateway.md)。本轮实现 OpenAI-compatible Chat Completions、Embeddings 和按实际需要的 Transcriptions；协议编码复用 Pydantic AI 与官方 SDK。禁用 SDK 的叠加自动重试，由一个执行入口管理。

模型名作为原样字符串传递。模型目录的条目不自动证明媒体或结构化能力。通用转换有明确记录，未支持的组合返回准确缺项，不能把文件路径、说明文字或空向量伪装成媒体/向量输入。

## 生命周期与范围

本轮采用同步等待的前台产品操作，可流式返回；没有后台恢复承诺。先停止接纳、结束操作与媒体子进程，再关闭模型连接、存储、观测和实例锁。

DBOS 继续保留后续接入方向，本轮不创建占位队列或永久通用调用回执。成功结果按产品生命周期保存；临时 trace、失败诊断与实验脚本各自清理。
