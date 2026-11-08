# 先替换责任结构，再在完整使用链上关闭缺陷

## 开工目标与授权

交付一个现行、可实际使用的 Nous：普通 SDK 可嵌入；显式宿主持有 DBOS；正文、标注和快照的完整用例控制事务；输入选择不丢失；已完成计算可独立恢复；日志与资源按实例归属；检索给出有用原文并能够管理派生资产。

基线为 `f724bc92d522372489d15fb6f0a16e436f819d5b`。编码者读取当前工作树、本地调用／数据流结果和本包；已有更新按真实责任吸收。不要回退开发者改动来套用基线行号。按当前工作树实施，保留独立研究与开发者不冲突修改。

产品版本保持现有 `0.1.0`。现行内部接口和数据形状可直接替换，不维护旧内部工作流、旧表格式或转发导入。开发者自己的数据目录不自动重建；需要新结构时明确选新的开发实例。有效用户文稿修订和精确引用按产品语义保留。

## 主计划的依赖顺序

| 工作单元 | 前置依赖 | 可见的完整结果 |
| --- | --- | --- |
| 统一业务合同与事务参与 | 无；先读当前源码和实际本地图谱 | 一次写入／采用能由真正负责者完整组合，不再调用反向事务钩子 |
| 生成、材料与正式结果顺流 | 前一项确立引用与采用边界；内容和媒体可协同推进 | 同一材料各阶段一致，成功计算可重开恢复 |
| 前台维护迁到新业务能力 | 新事务参与、生成／来源合同 | 无 Job 配方的前台标注与文稿维护可完整使用 |
| 显式持久宿主与领域工作流 | 前台维护、共同执行生命周期 | 保存入队、关闭重开、取消／重试和准确停止 |
| Wave 发布、资产与交付 | 事务参与、内容投影、实际引用 | 自洽快照、变化单元复用、标题和相关片段、可用清理 |
| 入口、说明与缺陷闭合 | 对应负责模块完成；日志／编辑可提前并行 | 正常 CLI/SDK/媒体/Job 路径均使用新结构，无旧支撑残留 |

这是工作依赖，不是按文件数切出的任意阶段。每个单元必须迁移真实生产者和消费者，可以在同一集成分支连续完成；不要求每个中间编辑都能单独发布。禁止为了让中间状态全绿保留旧 API、新旧类型转换桥或两套持久格式。

在接口仍被其他单元使用时，由同一负责人统筹类型和采用合同；并行工作只分给状态互不冲突的责任。压缩或交接时保留当前已定接口、实际完成能力和下一项工作，使用现有 CURRENT／施工计划，不另建多层工作账本。

## 统一合同与事务参与落实到保存和采用

### 调整实际定义者

涉及：`cognition/writing/{operations,records,reading,drafts,contracts}.py`、`cognition/annotations.py`、`cognition/concept_contracts.py`、`cognition/maintenance/operations.py`、`wave/{snapshots,operations,contracts}.py`。

先按 [总体架构](../ARCHITECTURE.md)和[维护架构](../design/cognition/maintenance-architecture.md) 确定所有权：WrittenRecords 保持文稿事务核心；Annotation 准备提案采用值并提供 adopt_in；Wave 提供准备结果及 accept/promote_in；完整用例持有 WriteTx。

### 实施动作

1. 保留并统一 target/expected 的实例、域、对象和 head 核验；所有修改入口直接使用它。名称解析归 NamespaceOperations，不在新句柄复制可变名称。
2. 从 AnnotationOperations.adopt 提取真正的事务参与：事务外准备精确提案和绑定决定，事务内检查活动前置条件、创建／复用标注、更新活动指针和 generation，返回 AnnotationCommit。
3. 将严格手工采用和后台 superseded 处理移到完整用例，在一次事务中决定是否保留 inactive 结果。删除先失败再另开事务重试采用的路径。
4. 让维护用例在同一个 WriteTx 里调用 annotation adopt_in 与 enqueue_refresh_in；删除 on_activation 参数、闭包和相关专属支撑。
5. 将 Wave 的耗时 build 准备与正式接纳／当前提升分开，迁移认知 refresh 和清单 build。删除 publish_if。
6. 定义 MaintenancePlan，AfterWrite 仅由维护 owner 解释一次；RecordOperations 使用计划，finish_write 组合真实结果，不再重算全部请求布尔条件。

### 交给后续单元的接口

- `AnnotationPlan`、`AnnotationCommit`、`MaintenancePlan` 见维护专题；类型归认知，非 Job 模块。
- `prepare_adoption(...) -> PreparedAnnotationAdoption`、`adopt_in(tx, prepared) -> AnnotationCommit`；prepared 包含提案、正文和绑定决定，调用者不读标注私表。
- `async with prepare_build(...) as prepared`、`accept_in(tx, prepared) -> SnapshotDescriptor`、`promote_in(tx, ref)`；各自合同见 Wave 专题。
- `RecordOutcome` 与 `CommittedRecord` 保留正式引用、提交代次和部分完成语义。

### 完成条件

一次正式采用及它承诺的刷新入队共同提交／回滚；普通文稿操作不受无关模型阻断；同一标注重复采用不会重复 Concept 或 generation；已有未激活标注能按本次合法请求激活；领域条件不在基础模块通过回调执行。完整手写记录和前台人工提案采用先可用，不等待 DBOS 才验证这些能力。

## 生成与材料重构形成可恢复的实际语言操作

### 调整实际定义者

涉及：`models/{operations,computation,input,tasks,executor,prompts,contracts}.py`、`media/{contracts,preparation,analysis,audiovisual,documents}.py`、`content/{contracts,results,mappings,provenance,artifacts,text,text_cache}.py`、`application/preparation.py`、`cognition/sources.py`、`cognition/writing/{generation,drafts,tasks}.py`。

将大 models/operations 中变化原因不同的部分收进 models/generation；模块对外仍是完整生成和恢复。已有网络 SDK、转写端点、媒体解析库、数据库与 CAS 不重写。

### 实施动作

1. 以 SelectedInputs 替换 InputPlan 中不完整的 originals 保存。迁移接纳、slot、role、分片、fallback、preview 和各语言阶段，不让下游反推原始选择。
2. 把已确认 MIME／范围事实交给共同路线规划。直接结构化和第二步结构化都从同一 SelectedInputs 重新计划表示；不复用第一步的物理 group 作为新任务。
3. 将输出坐标映射移到表示生产者采用时登记。迁移确定性文本投影、PDF 抽文、音频／视频范围与写作来源，删除从消费者 InputPart 猜映射的 fallback。
4. 将现有原始响应保存能力收成完整生成的一部分；已有最终产物由所属 owner 先返回，已有响应直接解释，没有结果才准备远端请求。
5. TaskDefinition 的输出解释只接收实际输出、交付事实和语义合同，不再要求 PreparedGeneration 或 Provider 存在。执行合同与 Prompt 帮助说明分开。
6. 删除标注的 PublishedComputation 转存链；解释结果经 PreparedOutputs 一次发布，在提案采用事务保存，DBOS 之后只接收 ResultRef。
7. DraftStore 保存写作请求／精确 base／来源／时间／反馈等恢复上下文；重开后根据正式引用继续草稿或采用，不只靠 PendingWriting 异常对象。
8. MediaAnalysis 恢复使用已保存语义请求和实际覆盖，删除 _reuse_description 的音频特例和显示形状全等要求。
9. 保留正文无损读取、缓存线程边界、取消后的真实子进程结束；用新的输入路径覆盖实际消费者，不能把旧函数作为旁路保留。

### 交给后续单元的接口

Generation 的 run/produce/recover_response/prepare_adoption/adopt_in 见 [成功计算](../design/models/durable-computation.md)；SelectedInputs 与 SourceFacts 见媒体专题；正式结果及坐标映射见内容专题。字段由各责任文件定义一次，使用方显式导入，不在每个目录复制等价 dataclass。

生成模块不解释 Annotation、Journal 或 WorkContext 的业务字段。任务语义依赖与认识来源分别保存；维护 owner 可以固定语言任务而不把整份运行配置序列化。

### 完成条件

真实入口可完成生成、提案、草稿、采用；已保存响应在模型配置变化后仍能本地解释；描述成功／结构化失败可重开继续；扩展名不改视频选路；实际选择与引用未扩大；无关转写配置不影响文本；重复采用不反复发布同样字节；Markdown 引用定义与尾部仍完整。

## 前台维护成为后台执行的唯一业务实现

### 实施动作

将 AnnotationOperations 的计划准备、提案解释、采用和读取归并为 `cognition/annotation/` 内部完整能力。不要只移动大文件；语言任务与事务采用有明确内部接口，公共调用不再穿透 generation.executor.compiler。

MaintenanceOperations 只组织真正维护政策及跨 owner 提交。inline 直接执行 AnnotationPlan；删除为复用而构造 background recipe、临时 job_id 和 JSON 串行化的代码。具体计算 key 独立于调度记录，显式新的标注请求可使用新 Prompt，旧计算的重试仍沿用原计划。

先接通无 DBOS 的前台完整能力：手写记录、自动／显式标注、普通不召回 Note 的编辑与显式标注、草稿采用、反馈更正和刷新。后台随后只包装这些真实操作的检查点，不再构造另一套业务规则。

### 完成条件

相同输入与政策的前台／持久维护在正式领域结果上保持一致；所有请求缺项与冲突都由共同 owner 决定；没有未使用的统一门面，也没有 CLI 自己拼 propose + adopt 的第二条根操作链。

## 宿主重构完整接入真实维护

### 调整实际定义者

涉及：`runtime/{bootstrap,execution}.py`、新增实际宿主位置 `runtime/host.py`、`jobs/{runtime,bridge,operations,client,contracts,maintenance}.py`、`storage/database.py`、`cognition/maintenance/`、`transports/cli/jobs.py`、`observability/`。

### 实施动作

1. 普通 open_instance 退出 worker 启停；保持数据库、内容、模型和共同操作装配。导入普通 SDK 或 JobClient 不运行 DBOS 装饰器。
2. run_host 显式拥有 Runner 和 DBOS 生命周期。检查外部框架所有权后再注册具体工作流，配置实例构造时依赖完整；无 `_instances` 后补绑定。
3. 将具体两项工作流和认知 payload/outcome 放入 cognition/maintenance。Job 基础不导入认知类型；边界处一次序列化。领域操作直接在宿主循环执行，删除 RuntimeBridge 及其测试／文档专属支撑。
4. 将 Execution 的寿命追踪与业务名额分开；Job 控制／等待用 slot-free 范围，业务阶段用现有根 operation，持久工作跟踪包含框架最后检查点。
5. 保留同库事务 enqueue、BEGIN IMMEDIATE、独立有限连接池以及现有配置来源。所有控制 I/O 与资源结束使用本项目受管理池，不依赖 DBOS 可能关闭的默认 executor。
6. 实施明确停机顺序：停止接纳、等待／取消真实工作、完成 SDK 关闭、再关控制客户端和业务资源。把关闭期间需要的资格放在共同生命周期，不给每个业务函数加 stopping 例外。
7. 保留单一失败重试后继、原队列 resume、等待超时不取消，以及新的显式标注请求。工作版本由 SDK 部署机制管理，本项目不递增内部合同版本来保旧分支。
8. CLI runtime run 和 job run 使用同一宿主操作，SDK 示例展示 application(runtime) 的实际调用。

### 必须由本地确认的 SDK 接缝

确认 DBOS 在选定宿主循环执行、同步 launch 的调用位置、实际 Task 与最后检查点的寿命、宿主停止导致的可恢复状态、默认 executor 的归属、外部 DBOS 的拒绝和资源不被销毁。查看正式 SDK 代码和本地行为即可；不要为此创建第二套执行实现或扩展成全量框架测试。

本包选择的目标是显式宿主；若实际 SDK 接缝出现无法满足退出合同的真实阻塞，修改宿主的一个适配点并明确结果，不能私改 DBOS、编造 pause API，或恢复双循环兼容分支声称已完成。

### 完成条件

保存真实文稿并入队，短进程退出，显式宿主继续标注与刷新；关闭重开仍可继续；取消／resume／ERROR retry 含义正确；单业务名额等待不死锁；关闭时前台已接纳保存可完成同事务入队；没有资源在使用中被先关闭。

## Wave 重构完成真实发布与交付

### 实施动作

1. 把构建结果的完整输入身份、候选布局、映射和 Native 组件写入同一清单；SQL 采用之前留下的 final 目录恢复读取这个清单，禁止重造今天的 mapping。
2. 完成 prepare_build／accept_in／promote_in 的实际迁移；认知代次资格留在认知事务，清单导入也使用同一 Wave 采用能力。
3. 以不可变投影单元和快照成员关系替换每代完整 mapping 复制。变化依赖包括正文、标题、标注、来源与实际政策；批量读取复用已有内容和向量。
4. 为实际变化的 Native 输入重新构建，未变组件依据完整签名复用；冻结候选行映射必须跟组件一致。暂不能局部更新的 VCP／ANN 保留准确的完整构建，不加 Python 补偿检索。
5. 标题进入明确词项字段及语义投影；交付使用查询相关原文范围。保留分组、质量分／排名键和来源支持的原有数学含义。
6. 候选描述、反馈与 read 批量从新映射取得，不让完整 JSON 加载重新回来。
7. 实施显式清理和查询借用：不删当前、运行中、未完成工作或正式反馈仍需的资产；共享单元在最后引用消失后才回收。
8. 更新 README 清单与实际示例，无旧字段兼容。

### 完成条件

完整构建、查询、固定旧快照读取和清理均可使用；映射与索引同源；并发相同视图共用构建；只新增一篇时不重复解析和复制所有候选映射；标题可查；长文末尾答案在交付中可见；记录仍全量构建部分的实际代价。分词实验保持用户当前实现，不另改分析器。

## 日志与编辑按各自负责模块并行完成

日志工作按 [日志](../design/foundation/logging.md)与[执行](../design/foundation/execution.md)专题 执行，可以在宿主接入前先形成实例隔离。所有消费者迁移到同一事件归属方式，删掉 root／全局 structlog 变更和共享 LogRecord 修改。保留真实 partial、请求尝试与内容准备诊断。

编辑工作按 [认知命令](../design/interfaces/cognition-cli.md) 执行：编辑会话文件先创建，正式提交确认前不删；锁／取消／错误均保留；恢复始终使用原 expected。不要先在现有异常分支上增加五种错误再宣称修复。

这两个单元可以独立交付行为，但同样要更新当前入口、配置和文档，不留旧代码供其他入口继续绕过。

## 收尾以完整使用能力与真实删除为准

按本轮已知问题运行相关操作：锁占用和取消保稿、跨实例日志隔离、描述成功后的结构化恢复、视频真实类型一致选路、长文相关范围交付、标题召回、当前清单、来源追加及资产复用与清理，覆盖已有高价值场景和新增结构接缝。模型用受控 HTTP 服务时说明验证的是请求与恢复链；真实外部质量另行确认。Windows 路径、编辑器、FFmpeg 与 Native 目录释放由实际支持环境完成，不把 Linux 结果表述为全平台已测。

不机械跑所有历史测试；不以数量或全绿代替上述能力。保留真实算法与回归保护，迁移仍有效的外部行为场景，删除只保护旧 bridge、recipe、私有 helper 和旧 schema 的测试及专属脚手架。没有为测试新增的生产导出口。

最后通过本地调用工具检查旧路径是否仍有真实消费者：on_activation、publish_if、RuntimeBridge、workers=True、PublishedComputation、原 annotations/operations 导入口、从 consumer 推断映射和重复 root 日志装配。静态工具的 UNKNOWN 或错误解析按真实源码处理，不为了图漂亮修改业务。

同步运行基础、认知、模型、内容、媒体和 Wave 的负责专题、源码索引、README 与示例；合并生命周期规则后删除旧 bridge 专题与重复文档。后续认知工作见[工作中的认知使用](cognitive-use-next.md)。CURRENT 只写已完成能力和实际剩余工作。不要把这份实施过程永久复制进产品设计；目标合同在各负责专题中单独定义。
