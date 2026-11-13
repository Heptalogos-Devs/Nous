# Nous 应用与 Wave Native 架构

## 产品能力与本次范围

Nous 提供独立的内容处理与长期认知能力。实例、配置、CAS、Artifact、ProcessingResult、模型、Prompt、媒体和 Embedding 是共同基础。Runtime 装配类型化完整操作，CLI、程序内 SDK、本地宿主、HTTP、MCP 和 gRPC 使用同一操作目录。正文、处理结果、草稿和检索快照分别在所属事务中保存。

Wave 集合是检索输入与可重建投影。cognition 拥有正式文稿、版本、Concept、Work 与采用，按一致读事务捕获材料目录和本次知识视图；清单导入也调用同一个 Wave 构建能力。内容字节和精确选区由 Artifact/TextView 提供，共同材料读取负责领域身份、结构和正式关系。

## 调用方向

```text
进程内 SDK / CLI / 本地宿主 / HTTP / MCP / gRPC
  ↓
完整应用操作：配置、生成、媒体解释、向量、Wave、文稿、Work、查询/读取/跟随/整理
  ├─ 内容接纳/读取/结果采用 → ArtifactStore、ResultStore → CAS、Database
  ├─ 语言阶段 → 当前调用的任务合同与 Prompt → ModelExecutor → 连接与凭据 → 网关
  ├─ 确定性准备 → MediaPreparer → Pillow、PDFium、FFmpeg
  └─ Wave → 内容/向量 owner、确定性媒体抽文 → 冻结资产 → Rust Native
                                           └─ retrieval Engine / Snapshot
                                               ├─ 词项/ANN 索引
                                               ├─ VCP 数值闭包
                                               └─ Nous 数值扩展

组合根：生命周期与依赖装配
共同基础：配置快照、Clock、Execution、Database、CAS、错误、轻量观测
普通实例：open_instance → 业务 Runtime；明确要求的维护在调用中等待
显式宿主：run_host → 当前 Runtime 中的后台 Task → 同一领域标注/刷新操作
```

跨内容和媒体的批量准备归 application/preparation；content 拥有范围和结构，media 拥有确定性表示。MediaPreparer 不持有模型执行器；ModelExecutor 不读取业务数据库。应用操作可以调用其它具有明确业务作用的操作，但不相互递归、不从整个 Runtime 查找依赖。CLI 不创建候选配置或另一套模型/模板编译路径。

领域任务先确定共同处理的语义单元，再由模型路线规划该单元的实际表示。所选材料、实际输入与命名产物分别保存；内容读取统一展开原件及其已有表示，认知来源在此基础上保留正式修订和用途。领域名称见[领域词汇](../GLOSSARY.md)。

Journal 的事务保存核心负责正文和来源；完整 RecordOperations 用例在同一 WriteTx 中组织修订、草稿采用及反馈关联。提交后由维护 owner 执行或启动标注与刷新。NamespaceRef 表示稳定域身份，见[维护架构](design/cognition/maintenance-architecture.md)。

认知查询绑定自然语言、NousQL 条件、正式材料与阶段情境，形成 QueryIntent。WaveProgram 编译及执行拥有观测共享、候选联合与校准、排序和独立局部定位。any/all 与情境分支在候选和排名阶段组合，最终按原问题定位并使用整次交付预算。认知层不另行计算检索分数，也不根据已经交付的片段重排材料。

EvidenceDelivery 固定实际交付的材料、原文与精确范围。Steward 消费已保存证据，按用途生成独立的 Markdown 阅读稿；query/read/follow 与后续研究由外部 Agent 组织。阅读稿进入草稿和正式文稿时，由写作 owner 读取共同材料关系并采用。

## 状态所有权

| 状态 | 所有者 |
|---|---|
| 配置定义、来源、保存值和有效代 | Configuration |
| 配置修改的绑定与资源采用 | ConfigurationOperations，调用相应 owner |
| 不可变字节及 owner/slot 引用 | ContentStore |
| 原件、派生物与实际处理输入输出 | ArtifactStore、ResultStore |
| 领域材料读取、结构字段 schema 与结果的正式材料来源 | 共同材料 owner，按正式引用类型登记领域读取者 |
| 原件、离散选择、slot、role 和已确认类型 | SelectedInputs 与内容 owner；路线从原选择规划实际表示 |
| 当前调用的语言任务与完成结果 | models.generation；领域任务解释结果，多阶段由 MediaAnalysis 组织 |
| 模型请求、尝试、流与响应 | ModelExecutor，协议适配由成熟依赖承担 |
| 候选模型排序与按声明尺度评价 | models evaluation，复用模型连接、期限和尝试策略 |
| 模板内容与输入输出合同 | PromptCatalog 与具体任务定义 |
| 向量空间、已采用向量及输入映射 | Embedding owner |
| Wave 集合、不可变投影单元、组件清单和活动快照 | Wave owner；当前视图资格由认知用例决定 |
| 冻结矩阵、索引和单次查询观测 | Native，均为可重建计算资产 |
| 查询程序、候选/校准/排名及局部定位 | Wave，Native 持有数值值；Python 组织模型节点和正式内容读取 |
| Note/Journal 正文版本与来源 | cognition 文稿 owner，共享有限版本保存机制 |
| 正式 Concept 与标注绑定 | cognition 各负责操作，固定意义/正文版本 |
| 当前知识视图与反馈 | cognition，复用 Wave 发布与检索；Native 不写认知 |
| Work 完整修订、有效决定、依据与实际使用 | cognition work；情境选择固定本次实际使用的工作修订 |
| 精确检索交付与阅读稿 | retrieval 与 Steward 分别采用完成成果，共用 ResultStore 和材料来源 |
| 当前运行期后台操作 | Execution 持有普通 Task 及有界终态；维护请求由认知 owner 解释 |
| 标注与刷新业务 | cognition maintenance，前台和后台共用完整操作 |
| 实例日志与遥测出口 | Runtime 的 LoggingSession/Observability；根操作绑定临时上下文 |
| 编辑中未提交的正文 | CLI EditSession 持有恢复文件，正式保存确认后结束 |

## 事务与副作用

耗时解码、模型调用、索引构建与 CAS 发布在短写事务外。完整用例开启 WriteTx，同一成果及其引用、正文及其来源、标注采用或快照及其关系分别原子提交；认知调用者在快照发布事务中决定是否提升当前指针。

已保存的提案和写作成果可独立读取、采用或重新组装草稿。任务解释器通过 InterpretedOutputs 交付命名输出、选区和已知映射；结果采用保留这些事实及正式材料来源。草稿保存完整正文、精确基准及来源。Native 只接收已冻结的数组和快照，不打开业务 SQLite，也不从 Python 回调逐条加载数据。

普通查询默认返回内存值。显式保存、Steward 和后续使用需要正式证据时，先采用 EvidenceDelivery，并在同一事务保留需要的快照引用；之后的整理失败或取消保留完成引用。局部 key 属于该交付，读取不依赖全局可变的最近查询。

认知捕获决定本次组资格和正式上下文；Wave 依据各文档的真实计算输入复用投影单元，共享来源图在本次组装中合并。快照持有组上下文和单元成员，单元持有文档计算资料。Native 行号在 Wave 接缝一次转换为类型化候选、排名依据和交付片段，认知查询继续绑定正式修订，入口负责呈现。

## 资源与并发

CPython 3.14、uv 与当前成熟依赖继续使用；Rust 通过 PyO3/maturin 接入。现行 Native 工作区是复用来源，不是不可变接口。计算脱离 Python 后使用自有不可变内存和一个实例级有界 Rayon 池。

Execution 控制接纳、根操作、截止时间和真实 Task／worker 寿命；模型连接、文件读取、媒体转换和 Native 计算分别管理自身容量。Native 合作取消由 Rust token 实现，取消后等待实际计算退出再释放资产。

后台维护仅在当前 Runtime 启用时接纳，任务结束或进程退出后不再运行。关闭先停止接纳、等待或取消活动操作，并等待线程、子进程和 Native 计算实际退出，再关闭连接、存储和实例锁。已经提交的成果继续按正式引用读取。

数据库、内容、模型与 Wave 分别使用已解析配置。Rust 算法参数由其真实配置类型导出 schema/defaults，Python 配置目录承接；不保留两套手写默认值。

## 开发边界

现有 VCP 数值模块与 DTSC 保留。Wave 共享不可变投影单元，以同源清单绑定索引与映射；资产由借用、成员引用和明确清理管理。局部词项和向量以实际原文单元构建，布局改变形成新的计算输入；程序及读出参数改变不重建未变的内容资产。仍需全量构建的 Native 组件按真实依赖重算。分词保持开发者的独立实验。任意历史知情视图、Social/Ego/Dream/Episode 见[后续计划](plans/cognitive-use-next.md)。

现行代码可以按真实用途重新组织。没有外部兼容承诺的接口直接迁移，旧路径和专属支撑随替换删除。临时样本和诊断留在 temp，网关补丁与独立语音工具保持独立。

## 深模块与正文结构

具体迁移位置见 [操作所有权](design/foundation/operation-ownership.md)。Rust retrieval 拥有 Engine/Snapshot 与查询业务，PyO3 只做语言边界。纯 schema 模块由组合根显式装配，业务服务不通过隐含导入注册表。

Journal 正文、版本、概念标注、关系结构和支持来源见 [Journal](design/cognition/journal.md)。具体模块接入见 [认知架构](design/cognition/integration.md)。正文保存与索引发布分别完成；current 查询按 [视图合同](design/cognition/current-retrieval.md)捕获一致代次，不依赖旧图过滤补偿。
