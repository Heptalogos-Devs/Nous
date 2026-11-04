# Nous 源码索引

按要修改的能力查找负责者，再沿实际调用链阅读。产品边界和当前进度见[架构](../../docs/ARCHITECTURE.md)与[当前工作](../../docs/CURRENT.md)；各专题合同位于 [docs/design](../../docs/design/)。

| 要处理的能力 | 位置与职责 |
|---|---|
| 实例启停与装配 | [runtime/bootstrap.py](runtime/bootstrap.py)：`initialize_instance`、`open_instance`、`Runtime`；初始化收集数据库表，运行时按生命周期关闭资源。 |
| 实例身份、路径和占有 | [runtime/instance.py](runtime/instance.py)：`InstanceLocator`、`InstanceLock`、`inspect_instance`；[runtime/paths.py](runtime/paths.py)：`ResolvedPaths`；[runtime/bindings.py](runtime/bindings.py)：持久资源身份与位置核对。 |
| 配置定义与来源 | [configuration/definitions.py](configuration/definitions.py)：`NousConfig` 和基础字段；[configuration/metadata.py](configuration/metadata.py)：`ConfigModel`、`setting`；[configuration/service.py](configuration/service.py)：来源合并、`ConfigSnapshot`、字段目录与原子文件保存。模型、媒体、内容的字段分别在 [models/configuration.py](models/configuration.py)、[media/configuration.py](media/configuration.py)、[content/configuration.py](content/configuration.py)。 |
| 配置变更与生效 | [application/configuration.py](application/configuration.py)：`ConfigurationOperations`、`open_configuration_operations`，共同处理在线/离线变更、路径与数据绑定、即时应用及待重启状态。 |
| 操作、期限与关闭 | [runtime/execution.py](runtime/execution.py)：`ExecutionManager`、`OperationContext`，管理嵌套操作快照、并发名额、期限、阻塞任务与关闭。 |
| 数据库和事务 | [storage/database.py](storage/database.py)：`Database`、`ReadTx`、`WriteTx`、SQLite 连接策略和基础表；各领域表由对应模块提供 metadata，初始化在 `runtime/bootstrap.py` 收集。 |
| CAS 字节及内容引用 | [storage/content.py](storage/content.py)：`ContentStore` 的流式准备、发布、读取与事务内引用；[storage/content_types.py](storage/content_types.py)：`BlobRef`。 |
| 原件、结果及来源 | [content/contracts.py](content/contracts.py)：共同引用、选区、实际表示及来源值；[content/artifacts.py](content/artifacts.py)：`ArtifactStore` 接纳、范围读取与本地借用；[content/results.py](content/results.py)：`ResultStore` 原子采用及结果视图；[content/provenance.py](content/provenance.py)：只遍历相关祖先的来源查询；[content/schema.py](content/schema.py)：规范化输入组、parts、origins 与输出关系。 |
| Markdown 正文与块 | [content/text.py](content/text.py)：`TextView`、`BlockRef`、源字节范围和完整块分片；通过 `ArtifactStore.text_view/read_blocks` 读取。 |
| 模型连接与凭据 | [network/__init__.py](network/__init__.py)：`ModelConnections` 管理协议客户端和网络资源；[credentials/__init__.py](credentials/__init__.py)：`Credentials` 解析 env/file 引用。 |
| 模型请求及任务 | [models/input.py](models/input.py)：joint/逐批语义组与按选中路线重规划；[models/tasks.py](models/tasks.py)：静态任务、来源键校验与有名产物；[models/annotations.py](models/annotations.py)：概念出现/路径提案、正文绑定与公开标注读取；[models/executor.py](models/executor.py)：当前请求、切换时调用准备回调及流；[models/policy.py](models/policy.py)：请求尝试、路线缺项、错误分类与期限；[models/operations.py](models/operations.py)：共同 prepare_group/run_group、按需转写、preview/probe 与实际结果采用。 |
| Prompt | [models/prompts.py](models/prompts.py)：`PromptCatalog`、`PromptCompiler`；[models/resources](models/resources/)：包内模板与 manifest，实例可覆盖。 |
| 媒体准备和解释 | [media/preparation.py](media/preparation.py)：`MediaPreparer` 的确定性引用计划、单批物化与派生；[media/contracts.py](media/contracts.py)：`PreparedSource`、`PreparedBatch`；[media/documents.py](media/documents.py)：图片/PDF；[media/audiovisual.py](media/audiovisual.py)：FFmpeg/ffprobe；[media/analysis.py](media/analysis.py)：`MediaAnalysis` 的三种策略与部分结果。 |
| 文本 Embedding | [models/embeddings.py](models/embeddings.py)：所有入口共用的 `encode_batches` 与协议校验；[models/embedding_schema.py](models/embedding_schema.py)：纯表与规范行唯一键；[models/embedding_store.py](models/embedding_store.py)：空间绑定及规范行选择；[application/embedding.py](application/embedding.py)：批量采用、临时 query 编码、`VectorRef` 与 CAS mmap 行读取。 |
| Wave 检索与快照 | [wave/contracts.py](wave/contracts.py)：清单、路径、支持组、固定候选池和显式查询向量；[wave/assets.py](wave/assets.py)：正文/标注、来源与冻结数组；[wave/snapshots.py](wave/snapshots.py)：构建发布、持久映射、打开与 Native 生命周期；[wave/query.py](wave/query.py)：查询信号、范围和结果物化；[wave/operations.py](wave/operations.py)：完整用例组合；[wave/schema.py](wave/schema.py)：纯表；[wave/configuration.py](wave/configuration.py)：同源参数。 |
| Native 计算与索引 | [native/vcp](../../native/vcp/)：VCP 数值闭包、Thinking 与 DTSC；[native/retrieval](../../native/retrieval/)：冻结资产、快照持久化、观测、候选池、检索读出、反证、Thinking 及词项/向量索引；[native/python](../../native/python/)：NumPy/PyO3 转换、Rust 对象持有、取消和计算资源生命周期。 |
| 多轴时间 | [temporal/values.py](temporal/values.py)：时钟、域内坐标、未知值与解析；[temporal/coordinates.py](temporal/coordinates.py)：共同坐标校验；[temporal/domains.py](temporal/domains.py)：时间域持久绑定及 `TimeService.validate` 的已注册域检查。 |
| 错误、日志和遥测 | [common/errors.py](common/errors.py)：`NousError`；[common/operations.py](common/operations.py)：稳定根关联、已采用结果与异常上下文；[observability/instrumentation.py](observability/instrumentation.py)：操作/请求尝试、Trace 和指标 API；[observability/diagnostics.py](observability/diagnostics.py)：保留 JSONL 操作日志读取；[observability/assembly.py](observability/assembly.py)：独立终端/文件呈现及 SDK 装配。 |
| CLI、语言与呈现 | [transports/cli/__init__.py](transports/cli/__init__.py)：实例管理、选项和错误出口；[transports/cli/tools.py](transports/cli/tools.py)：模型/内容/媒体工具；[transports/cli/wave.py](transports/cli/wave.py)：Wave 参数转换与结果呈现；[presentation/__init__.py](presentation/__init__.py)：翻译、状态和时间呈现。 |
| 已有回归 | [tests/python](../../tests/python/)：CLI 与时间、配置失败、内容事务和实例资源的行为测试。 |
