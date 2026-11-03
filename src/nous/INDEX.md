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
| 原件、结果及来源 | [content/contracts.py](content/contracts.py)：`ArtifactRef`、`ResultRef`、`SourceSelection` 等值；[content/store.py](content/store.py)：`ArtifactStore`、`ResultStore`；[content/schema.py](content/schema.py)：原件、输入、输出与结果关系。 |
| 模型连接与凭据 | [network/__init__.py](network/__init__.py)：`ModelConnections` 管理协议客户端和网络资源；[credentials/__init__.py](credentials/__init__.py)：`Credentials` 解析 env/file 引用。 |
| 模型请求及任务 | [models/contracts.py](models/contracts.py)：输入、输出和任务合同；[models/executor.py](models/executor.py)：资源选择、准备、调用、有限尝试和流；[models/operations.py](models/operations.py)：接纳、准备、执行及结果采用。 |
| Prompt | [models/prompts.py](models/prompts.py)：`PromptCatalog`、`PromptCompiler`；[models/resources](models/resources/)：包内模板与 manifest，实例可覆盖。 |
| 媒体准备和解释 | [media/preparation.py](media/preparation.py)：`MediaPreparer`、分批和派生；[media/documents.py](media/documents.py)：图片/PDF；[media/audiovisual.py](media/audiovisual.py)：FFmpeg/ffprobe；[media/analysis.py](media/analysis.py)：`MediaAnalysis` 的三种策略与部分结果。 |
| 文本 Embedding | [models/embeddings.py](models/embeddings.py)：空间语义、probe 和向量执行；[models/embedding_store.py](models/embedding_store.py)：已用空间绑定与向量关联；[application/embedding.py](application/embedding.py)：`EmbeddingOperations` 和原子采用。 |
| 多轴时间 | [temporal/values.py](temporal/values.py)：时钟、域内坐标、未知值与解析；[temporal/coordinates.py](temporal/coordinates.py)：共同坐标校验；[temporal/domains.py](temporal/domains.py)：时间域持久绑定。 |
| 错误、日志和遥测 | [common/errors.py](common/errors.py)：`NousError`；[observability/instrumentation.py](observability/instrumentation.py)：轻量操作、Trace 和指标 API；[observability/assembly.py](observability/assembly.py)：日志与 OpenTelemetry SDK 装配。 |
| CLI、语言与呈现 | [transports/cli/__init__.py](transports/cli/__init__.py)：实例管理、选项和错误出口；[transports/cli/tools.py](transports/cli/tools.py)：模型/内容/媒体工具；[presentation/__init__.py](presentation/__init__.py)：翻译、状态和时间呈现。 |
| 已有回归 | [tests/python](../../tests/python/)：CLI 与时间、配置失败、内容事务和实例资源的行为测试。 |
