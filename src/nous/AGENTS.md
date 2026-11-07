# src/nous 开发指引

按任务使用 [INDEX.md](INDEX.md)、CodeGraph 和实际调用链定位负责者，再阅读相关设计与实现。功能缺失或错误直接修到负责者，补强共同基础设施，不在使用处局部绕过、添加平行实现，或用包装层维持已经不合适的结构。

下列规则提炼现行设计中的接入方式与关键约束，详细合同以对应设计专题为准。需要改变职责或公共契约时，同步修改负责模块、所有实际调用方和对应设计。

## 实例与装配

- 通过 `initialize_instance` 和 `open_instance` 管理实例。路径与身份使用 `InstanceLocator`、`ResolvedPaths` 和实例绑定；普通打开不得创建替代数据库或绕过资源锁。
- 组合根显式构造依赖，构造完成的服务即可使用，不在发布后回填依赖。新增表由具体 owner 提供，在显式初始化时统一收集。
- 资源纳入运行时生命周期。关闭时先停止接纳并结束操作、阻塞工作和媒体子进程，再关闭模型连接、存储、观测及实例锁；不留下运行时无法等待的后台工作。

## 配置

- 配置字段在负责模块定义并接入 `NousConfig`。来源合并、有效快照和文件保存由 `Configuration` 处理；在线与离线修改都使用 `ConfigurationOperations`。
- 业务代码不另读环境变量、不维护默认值副本、不另写实例、时间或向量空间的绑定规则。应用操作调用实际 owner 完成绑定检查和采用。
- 一次操作使用确定的配置快照。配置变更的即时生效或待重启行为遵循现有合同，不能由某个入口自行决定。

## 执行与事务

- 业务操作进入 `ExecutionManager.operation`。内部阶段继承当前快照和期限；阻塞工作使用 `ExecutionManager.blocking`，可中止的 Native 工作传入 `on_cancel` 并等待实际退出；后台任务使用 `spawn`。嵌套操作不得重复获取全局并发名额。
- SQLite 使用 `Database.read` / `write` 及调用方的事务。耗时解析、网络请求和字节发布放在写事务之外；已有事务内不再调用另开事务的操作。
- 一次正式采用在同一短事务中提交相关结果和引用。跨 owner 协作使用接收现有 `WriteTx` 的 `*_in` 方法，不通过逐个提交再补偿来拼接原子性。

## 内容与来源

- 字节存储使用 `ContentStore`，原件身份使用 `ArtifactStore`，处理结果使用 `ResultStore`。CAS 摘要表示字节，不能代替来源身份；同样字节的两次接纳可以有不同 Artifact。
- 跨 owner 采用时先发布字节，再在同一 `WriteTx` 中提交各自记录。Embedding 首次空间绑定、结果和向量关系一起提交。
- 内容接纳、媒体准备、模型请求与结果采用使用共同的引用和值类型，不另造平行的文件、来源或结果结构。选区、实际覆盖和派生关系按负责专题保持一致。
- 原件操作位于 `content.artifacts`，结果采用位于 `content.results`；通过 `ResultStore.provenance` 读取相关祖先与终端根。血缘根集合与关系的独立支持组分别表达，不从共同使用过的材料推断独立支持。
- Markdown 结构使用 `ArtifactStore.text_view/read_blocks` 与共同 `TextView`。块键固定正文及解析 profile，模型选段与概念标注使用这些精确引用。
- 对于自然语言处理：禁止依据某语言关键词、词表、语言类别或内容枚举，或使用正则表达式等类似方式处理。Lexical 全文检索和用户明确要求的翻译、摘要等领域处理，继续按各自合同执行。
- Note/Journal 的正文版本和引用由 cognition 文稿 owner 采用；模型写作先保存 ProcessingResult 与草稿，再在独立事务采用正式文稿。Concept 意义版本和正文标注也由 cognition 采用，固定各自的版本。

## Prompt、模型与媒体

- Prompt 选择与编译使用 `PromptCatalog` / `PromptCompiler`；静态 `TaskDefinition` 负责任务输出与来源语义；生成、转写、预览和探测经 `GenerationOperations`，协议执行经 `ModelExecutor`。媒体准备与解释使用 `MediaPreparer` / `MediaAnalysis`。
- 网关连接和凭据由 `ModelConnections`、`Credentials` 管理。协议编码、媒体解析和转换采用现有依赖，不重复实现客户端、重试或解析器。
- 根据明确协议、能力和任务配置选择行为，模型名原样传递，不按模型名或厂商添加业务分支。New API 的部署和管理属于开发接入，不进入产品逻辑。
- 媒体准备负责确定性解码、选择、转换及无字节引用计划，消费前只物化一个有界批次；媒体解释负责转写、描述与结构化的策略和阶段衔接，复用共同生成操作。不要让 CLI、模型客户端或后续认知领域各写一套准备逻辑。
- 语义材料集合由 `models.input` 规划，普通任务使用 joint；独立逐批执行由明确请求或任务定义选择。实际表示 MIME 与源 MIME 随计划保存。
- 模型执行不访问业务数据库；结果采用由应用操作完成。模型已返回成功而保存失败时，处理采用失败，不重新请求模型。
- 文本向量通过 `EmbeddingOperations.ensure/read` 批量获取和复用；依空间、用途和精确输入读取正式向量行，不绕过 owner 自建模型客户端或向量缓存。
- run、ensure、probe 和临时查询共用 `EmbeddingExecutor.encode_batches`；临时查询使用 `encode_query`，已存向量使用 `VectorRef/read_vector`。表的唯一键选择规范已采用行，网络请求不持有实例级复用锁。
- LLM 任务分工、实际输入、结果处理和提示词编写遵循 [LLM 使用与提示词设计](../../docs/agents/llm-use.md)。按模型实际收到的信息判断输出，确定性业务规则由系统负责。

## Wave 与 Native

- 认知域在一致读事务中提供正式文稿、概念、标注和来源视图；`WaveOperations` 接收类型化 `CorpusView`，与清单导入共用资产构建和快照发布。PDF 检索复用 `MediaPreparer`。交给 Native 的只有冻结数组和可重建资产。
- 构建发布不可变快照；查询、Thinking 和原文读取消费该快照。原始来源序列、候选曲线文稿和独立证据根是不同身份，派生材料不能重复贡献独立事实。
- Wave 参数由 Rust 导出并在 `WaveConfig` 接入共同配置；Python 不实现备用检索算法。Native 计算池、取消令牌和数据边界见 [native/AGENTS.md](../../native/AGENTS.md)。

## 时间、错误与观测

- 时间使用 `temporal` 的域、时钟和值类型，坐标域通过 `TimeService.validate` 校验注册，保留未知值、精度和坐标域。媒体页码、帧号和相对时段不能当成现实时间。
- 错误使用 `NousError`，由入口负责呈现。不要用空结果、默认数据或成功状态掩盖实际失败。
- 根操作关联、尝试与已完成引用由共同操作上下文携带；请求使用 `models.policy.attempt_scope`。取消保持原异常类型到入口，诊断通过 `runtime logs --operation` 读取现有 JSONL。
- 基础埋点使用标准 logging 和 `observability.instrumentation`；SDK 装配留在 `observability.assembly`。基础模块不因埋点反向依赖完整 SDK、exporter 或业务装配。
- 日志、错误和正式结果不得包含凭据。

## 入口与职责

- CLI 负责参数转换、调用完整操作和呈现，业务规则留在共同操作中。语言选择与文案使用 `presentation`。
- 用户调用应完成有意义的操作，不要求用户逐个执行内部 prepare、publish、attach 或编译步骤。
- 后续入口复用同一应用操作，不把所有内部方法逐一暴露为工具，也不为尚未开展的 HTTP、MCP、SDK 或持久任务建立占位实现。
- 新能力先检查现有 owner 和已选依赖，按真实高内聚职责组织代码，不为每个术语创建一层类、文件或包。

## 维护源码知识

职责、公共契约、配置归属或实际调用方式改变时，更新 [INDEX.md](INDEX.md)、本文件中对应的接入约束和受影响的设计专题，移除失效指引。INDEX.md 用于找到负责位置，本文件说明如何接入，两者都不复制完整设计或累积实现历史。
