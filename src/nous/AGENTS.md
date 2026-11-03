# src/nous 开发指引

本目录遵循仓库根目录的 `AGENTS.md`。涉及已有基础能力时，按任务使用 [INDEX.md](INDEX.md) 和 CodeGraph 定位负责者及实际调用方；设计发生变化时同步对应专题。

## 接入现有能力

- 实例由 `initialize_instance` 和 `open_instance` 管理。使用 `InstanceLocator`、`ResolvedPaths` 和实例绑定处理路径与身份；普通打开不得创建替代数据库或绕过资源锁。
- 配置字段在负责模块定义并接入 `NousConfig`，来源、快照和文件保存由 `Configuration` 处理；在线及离线变更使用 `ConfigurationOperations`。业务代码不得另读环境变量、维护默认值副本或另写绑定规则。
- 运行中的业务操作使用 `ExecutionManager.operation`；内部阶段继承当前快照和期限，阻塞工作使用 `ExecutionManager.blocking`，后台任务使用其 `spawn`。不得为嵌套操作重复获取全局名额，或留下运行时无法等待和关闭的任务。
- SQLite 通过 `Database.read` / `write` 和调用方的事务工作；初始化时由组合根收集新表。耗时解析与网络请求放在写事务之外，不在已有事务内调用另开事务的操作。
- 内容使用 `ContentStore`、`ArtifactStore` 和 `ResultStore`。跨所有者采用时先发布字节，再在同一 `WriteTx` 内调用各自的 `*_in`；Embedding 首次绑定、结果和向量关系一起提交。
- Prompt、模型和媒体分别使用现有的 `PromptCatalog` / `PromptCompiler`、`GenerationOperations` / `ModelExecutor`、`MediaPreparer` / `MediaAnalysis`。网关连接与凭据交给 `ModelConnections` 和 `Credentials`；不得按模型名或厂商分支，也不得重复实现协议客户端、重试和媒体解析。
- 时间使用 `temporal` 的域、时钟和值类型；保留未知、精度和坐标域，不把媒体页码或相对时段当成现实时间。错误使用 `NousError`；基础埋点使用标准 logging 与 `observability.instrumentation`，SDK 装配留在 `observability.assembly`，日志和结果不得包含凭据。
- CLI 负责参数转换与呈现，业务规则留在共同操作中；语言选择和文案使用 `presentation`。需要新能力时先检查索引中的现有所有者和已选依赖，不建立平行实现。

## 维护源码知识

对重要类和方法，用简短注释说明职责、状态边界或不明显的不变量；不要逐行复述实现。代码注释不记录已退役源码路径或复用过程。职责、公共契约或配置归属变化时更新 [INDEX.md](INDEX.md) 和受影响设计，移除失效指引。
