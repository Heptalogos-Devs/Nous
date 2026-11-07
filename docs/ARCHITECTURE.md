# Nous 应用与 Wave Native 架构

## 产品能力与本次范围

Nous 提供独立的内容处理与长期认知能力。实例、配置、CAS、Artifact、ProcessingResult、模型、Prompt、媒体和 Embedding 是共同基础。普通 Runtime 提供进程内完整操作，显式宿主拥有持久执行。事务、材料选择、成功计算恢复与检索资产按各自真实责任组织，迁移范围见[主计划](plans/architecture-rework.md)。

Wave 集合是检索输入与可重建投影。cognition 拥有正式文稿、版本、Concept 与采用；按一致读事务提供类型化 CorpusView，清单导入也调用同一个 Wave 构建能力。内容字节和精确选区继续由 Artifact/TextView 提供。

## 调用方向

```text
进程内宿主 / CLI / 后续 HTTP、MCP、远程 SDK
  ↓
完整应用操作：配置、生成、媒体解释、向量、Wave、认知文稿/采用/查询
  ├─ 内容接纳/读取/结果采用 → ArtifactStore、ResultStore → CAS、Database
  ├─ 语言阶段 → 任务合同 + 冻结 Prompt → ModelExecutor → 连接与凭据 → 网关
  ├─ 确定性准备 → MediaPreparer → Pillow、PDFium、FFmpeg
  └─ Wave → 内容/向量 owner、确定性媒体抽文 → 冻结资产 → Rust Native
                                           └─ retrieval Engine / Snapshot
                                               ├─ 词项/ANN 索引
                                               ├─ VCP 数值闭包
                                               └─ Nous 数值扩展

组合根：生命周期与依赖装配
共同基础：配置快照、Clock、Execution、Database、CAS、错误、轻量观测
普通实例：open_instance → 业务 Runtime 与受限 Job 客户端
显式宿主：run_host → 同一应用循环上的 DBOS → 领域标注/刷新操作
```

跨内容和媒体的批量准备归 application/preparation；content 拥有范围和结构，media 拥有确定性表示。MediaPreparer 不持有模型执行器；ModelExecutor 不读取业务数据库。应用操作可以调用其它具有明确业务作用的操作，但不相互递归、不从整个 Runtime 查找依赖。CLI 不创建候选配置或另一套模型/模板编译路径。

Journal 的事务保存核心不持有标注与检索对象；完整 RecordOperations 用例持有 WriteTx，并消费维护 owner 一次解释的 MaintenancePlan。Annotation 与 Wave 通过事务参与方法接纳正式状态，维护入队与承诺的领域变化共同提交。作用域句柄绑定稳定 Namespace 并复用基础服务，见[维护架构](design/cognition/maintenance-architecture.md)。

## 状态所有权

| 状态 | 所有者 |
|---|---|
| 配置定义、来源、保存值和有效代 | Configuration |
| 配置修改的绑定与资源采用 | ConfigurationOperations，调用相应 owner |
| 不可变字节及 owner/slot 引用 | ContentStore |
| 原件、派生物与实际处理输入输出 | ArtifactStore、ResultStore |
| 原件、离散选择、slot、role 和已确认类型 | SelectedInputs 与内容 owner；路线从原选择规划实际表示 |
| 一次语言任务、成功响应与结果恢复 | models.generation；领域任务解释其语义，多阶段由 MediaAnalysis 组织 |
| 模型请求、尝试、流与响应 | ModelExecutor，协议适配由成熟依赖承担 |
| 模板内容与输入输出合同 | PromptCatalog 与具体任务定义 |
| 向量空间、已采用向量及输入映射 | Embedding owner |
| Wave 集合、不可变投影单元、组件清单和活动快照 | Wave owner；当前视图资格由认知用例决定 |
| 冻结矩阵、索引和单次查询观测 | Native，均为可重建计算资产 |
| Note/Journal 正文版本与来源 | cognition 文稿 owner，共享有限版本保存机制 |
| 正式 Concept 与标注绑定 | cognition 各负责操作，固定意义/正文版本 |
| 当前知识视图与反馈 | cognition，复用 Wave 发布与检索；Native 不写认知 |
| 持久 Job 与步骤状态 | DBOS，经通用 jobs 适配；具体维护计划、工作流与结果由认知 owner 负责 |
| 标注与刷新业务 | cognition maintenance，前台和后台共用完整操作 |
| 实例日志与遥测出口 | Runtime 的 LoggingSession/Observability；根操作绑定临时上下文 |
| 编辑中未提交的正文 | CLI EditSession 持有恢复文件，正式保存确认后结束 |

## 事务与副作用

耗时解码、模型调用、索引构建与 CAS 发布在短写事务外。完整用例开启 WriteTx，结果、引用、空间/快照关系及承诺的持久入队由参与者在同一事务完成；Annotation.adopt_in 与 Wave.accept_in/promote_in 不接受反向业务回调。

已有正式提案先读取采用，已有成功响应直接按保存的语义解释，只有缺少结果时才准备新请求。PreparedOutputs 复用已发布字节，写作草稿持久保存精确 base、来源与请求上下文。生产者在采用表示时登记坐标映射，消费方沿映射选择原文。Native 只接收已冻结的数组/快照，不打开业务 SQLite，也不从 Python 回调逐条加载数据。

## 资源与并发

CPython 3.14、uv 与当前成熟依赖继续使用；Rust 通过 PyO3/maturin 接入。现行 Native 工作区是复用来源，不是不可变接口。计算脱离 Python 后使用自有不可变内存和一个实例级有界 Rayon 池。

Execution 控制接纳、根操作、截止时间和真实 Task／worker 寿命；业务名额与可跟踪控制等待分开。Native 合作取消由 Rust token 实现，取消后等待实际计算退出再释放资产。Native 不另起守护进程。

普通 open_instance 不启动 DBOS。run_host 在显式拥有的应用循环注册并运行持久工作；领域工作直接调用同一 Runtime。停止先结束已接纳业务与最后检查点，再关闭框架、客户端和业务资源。Job 控制 I/O 使用受管理池，取消请求与真实工作结束分别表达。

数据库、内容、模型与 Wave 分别使用已解析配置。Rust 算法参数由其真实配置类型导出 schema/defaults，Python 配置目录承接；不保留两套手写默认值。

## 开发边界

现有 VCP 数值模块与 DTSC 保留。Wave 共享不可变投影单元，以同源清单绑定索引与映射；资产由借用、成员引用和明确清理管理。仍需全量构建的 Native 组件按真实依赖重算。分词保持开发者的独立实验。WorkContext—Wave—Steward—实际使用—情境反馈为后续主线；完整 NousQL、任意历史视图、Social/Ego/Dream/Episode 见[后续计划](plans/cognitive-use-next.md)。

现行代码可以按真实用途重新组织。没有外部兼容承诺的接口直接迁移，旧路径和专属支撑随替换删除。临时样本和诊断留在 temp，网关补丁与独立语音工具保持独立。

## 深模块与正文结构

具体迁移位置见 [操作所有权](design/foundation/operation-ownership.md)。Rust retrieval 拥有 Engine/Snapshot 与查询业务，PyO3 只做语言边界。纯 schema 模块由组合根显式装配，业务服务不通过隐含导入注册表。

Journal 正文、版本、概念标注、关系结构和支持来源见 [Journal](design/cognition/journal.md)。具体模块接入见 [认知架构](design/cognition/integration.md)。正文保存与索引发布分别完成；current 查询按 [视图合同](design/cognition/current-retrieval.md)捕获一致代次，不依赖旧图过滤补偿。
