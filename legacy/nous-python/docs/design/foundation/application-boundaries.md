# 完整应用操作与共同输入边界

应用层提供用户可直接完成的操作。`OperationDefinition` 集中请求、结果、事件类型与名称；组合根将定义与实际 owner 绑定，`Application.call` 供 SDK、本地 CLI 和运行中宿主共同调用。业务规则位于各能力的完整操作，入口负责参数、输入绑定与结果交付。

`CallContext` 携带本次请求的执行上下文、`InputReader` 和事件出口。请求取得外部输入、执行、保存和返回共用期限；内部阶段继承同一配置快照。宿主由客户端在请求开始时主动绑定文件和标准输入，正式来源使用原始显示名与已接纳 Artifact，不使用宿主暂存路径。

`runtime.app.list()` 和 `runtime.app.describe(name)` 从同一操作定义提供简介及请求、结果、事件 Schema；CLI 使用 `nous operations list` 和 `nous operations describe <名称>` 查询。配置、Prompt 和时间各有独立请求，离线配置入口与运行中宿主使用同一组操作绑定。

`Saved` 事件在正式提交后携带带类型标识的成果引用。事件、运行期操作状态、错误中的 `completed_refs` 和日志使用相同的引用事实。前台观察者无法接收事件时结束调用，已提交成果仍可按引用读取；后台维护使用独立的事件出口。

## 共同能力

| 能力 | 负责者 |
|---|---|
| 输入绑定与接纳 | `application.inputs.InputReader`，统一内联文本、外部输入、Artifact 和命名 Result 输出 |
| 原选择与实际输入 | `content.contracts.SelectedInputs`、`InputGroup`、`ContentSelection` |
| 内容和成果 | ArtifactStore、ResultStore；`ResultFacts` 与命名输出在短事务中采用 |
| 类型探测与表示准备 | MediaPreparer；原选择、真实模态、实际表示和范围分别保留 |
| 任务与模型调用 | TaskDefinition、GenerationOperations、ModelExecutor；任务负责语义和输出解释 |
| 向量与检索 | EmbeddingOperations、WaveOperations 与 Native；认知领域负责正式视图和反馈 |
| 运行期资源 | ExecutionManager 跟踪请求、当前进程后台 Task、线程和受控进程 |

`SourceTarget` 的分支是内联文本、Artifact、Result 命名输出和 `ExternalInput`。每个完整操作声明所需请求；认知来源另用 `SourceChoice` 承载正式修订与用途。调用期从接纳后的原件和选区计划实际表示；同一字节的不同原件保留各自身份。

纯 Prompt 预览进行输入准备和编译，不发送语言或转写请求。模型或程序形成完整输出后保存成果；后续领域加工失败时读取已保存成果并显式重新发起加工。读取结果不依赖旧模型配置。来源与映射见[内容记录](content-records.md)，运行期控制见[宿主与操作](../interfaces/jobs-and-host.md)。

可读标识符由类型前缀和括号内六个单元组成；内部使用 UUID。修订号写在右括号之后，例如 `jrn(...)@3`。在命令行中将完整标识符作为一个参数引用。
