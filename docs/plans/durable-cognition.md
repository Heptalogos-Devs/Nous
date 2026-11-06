# 可靠认知操作与持久维护

## 产品目标

让正式认知写入具有一致的身份、来源、版本与执行语义，并让保存后的标注和检索维护在长期宿主中持续完成、在重启后继续。当前基线为 `8153ae2`；本轮沿已完成的 Journal Maintenance 继续，而非重建 Note、Journal、Concept 和 Wave。

完整用户路径为：创建/修订文稿，取得正式引用和可观察的维护工作；保存请求结束后维护继续运行；关闭、重新打开同一实例后恢复；可以查询工作、处理失败、读取标注和准确的当前检索结果。人工直接维护、程序调用与持久执行共享领域操作。

## 已有能力与需要保留的责任

`WrittenRecords` 拥有文稿提交，`RecordOperations` 拥有完整用例，`WritingOperations` 区分 journalize/edit/rewrite/reflect。不可变 Artifact、来源分组、ResultStore、Concept 意义版本、Annotation、Namespace generation、Wave 冻结视图、分组召回均继续使用。Native 的 VCP、RiverMemo、Thinking、词项/向量召回与分数含义不因接入 Job 改变。

保存成功与后续维护结果分别报告，精确引用保持历史版本。Namespace 与实例身份不依赖名称。模型执行器保持供应商中立，访问既有统一网关，SDK 隐含重试继续关闭。

## 工作依赖与实施顺序

### 先完成稳定身份与来源合同

修改 `cognition/scope.py`、`services.py`、`runtime/bootstrap.py`、`writing/reading.py`、`concepts.py`、`retrieval/operations.py` 及 CLI 的作用域入口。绑定句柄只保留稳定 NamespaceRef。所有显式名称先由 Namespace owner 解析，再核对身份。

修改 `RecordOperations.replace/apply/archive/restore/set_recall` 及底层提交参数。target 与 expected 分别进入共同 owner，CLI 不再以 expected 替换 target。`expected` 为数字时，领域 owner 在目标对象内解释 ordinal；完整引用必须与 target 同属一对象。事务内再比较 current head。

在 `cognition/sources.py` 集中目前 `RecordReader.select_sources`、`WritingOperations._freeze`、手工 source_requests 和草稿/反馈对精确来源的解析。引入 RevisionSelection，迁移查询、读取、选段和反馈的生产者。内容范围的校验与归一化由 content/media 能力执行；认知 owner 负责正式修订归属与 current_basis。

### 统一原文读取、准备与视图修改

修改 `content/artifacts.py`、`text.py`、`text_cache.py`，将跨媒体批量准备从 `content/preparation.py` 移至 `application/preparation.py`。Model/Input、Embedding、Wave 构建和读取使用相同范围/结构能力。全文 rewrite 和 edit_target 明确使用无损原文，定义行、CRLF、尾部与块间原文保留。

缓存命中和未命中的 CPU 工作都纳入受管理的工作线程；纯结构缓存不急切复制所有块对象。读取上限约束实际选区，不能先读取大原件全体再裁掉。

标注采用拆分“取得/创建标注对象”和“更新活动指针”。是否推进 generation 由实际 current 可召回视图变化决定，覆盖 historical 和相同 adoption_key 重用。事务内应用变化，当前快照由 generation 判断是否更新。

### 完成 Runtime 执行、配置与资源基础

ConfigSnapshot 创建时完成校验并持有深度不可变的类型化视图；迁移 typed_values 的消费者，禁止在逐文稿 getter 重建全配置。配置变更进入共同 ExecutionManager；离线配置用例仍能独立使用轻量资源。

Execution 根操作绑定其 Runtime 的观测 provider、instance_id、startup_id，保留外部 W3C Trace 上下文。缓存、子进程、Native 与 CAS 收尾均服从实际完成状态，重复取消不能释放仍在使用的资源。

数据库统一调整读/写事务控制：读事务形成一致视图；写事务在任何依据查询前 BEGIN IMMEDIATE。删除 Python 非 legacy autocommit=False 与自定义 BEGIN 混用的路径。DBOS 连接通过相同已解析路径和配置建立，并由明确 owner 关闭。

### 接入真实持久维护

在 `jobs/` 实现 DBOS 生命周期、官方客户端适配、JSON 值合同、运行循环交接、Job 查询与控制。在 `cognition/maintenance/` 实现明确的标注和刷新业务，DBOS workflow 只组织可重放步骤。

采用同一 SQLite 文件中的 DBOS 系统表；在实例初始化或显式升级时用官方迁移建立，普通短命令只验证可用性。先注册命名类/实例及配置，再启动恢复；不逐 Namespace 注册 workflow。

`RecordOperations` 在正式正文 WriteTx 内登记明确请求的 Job，提交后返回 JobRef。annotation 和 refresh 分开，annotation 成功激活的事务再入队 refresh。既有前台 `AfterWrite` 行为保留为同一业务能力的执行选择。

冻结语言输入配方、Prompt 正文、路线和参数；成功计算保存可重读输出及描述；采用以持久工作/步骤的稳定业务键幂等。工作状态归 DBOS，正式产物归现有内容/认知 owner。

### 接通操作面并整理文档

实现 `open_instance(..., workers=True)`、`runtime.jobs`、`nous job list/read/wait/cancel/resume/retry` 和运行待办工作的命令。`resume` 用于可恢复的取消/超限状态，必须将工作送回原队列；失败终态 `ERROR` 的 `retry` 从确定的失败步骤 fork 出新的 JobRef，保持旧错误、前置步骤、冻结输入与结果身份。运行中的独立 Job 控制不 open_instance 第二个写入宿主，只连接同文件的 DBOS 管理客户端；其能力不能任意修改认知数据。

迁移 CLI、示例、动态任务注册、配置目录、包导出和有效测试。同步当前架构、状态、专题和后续路线。见 [迁移地图](../development/durable-cognition-migration.md)。

## 关键接口

```python
async with open_instance(locator, workers=True) as runtime:
    c = await runtime.cognition("research")
    outcome = await c.records.create_journal(
        "# 认识\n\n正文", after=AfterWrite(annotation="run", retrieval="refresh", execution="background")
    )
    for job in outcome.jobs:
        await runtime.jobs.wait(job)
    answer = await c.retrieval.query(CognitiveQuery(text="正文讨论的内容"))
```

该路径通过当前 SDK 与 CLI 调用同一业务能力。`AfterWrite.execution`、`RecordOutcome.jobs` 和 JobRef 已接入；完整调用见[接口](../design/interfaces/jobs-and-host.md)与[操作说明](../development/durable-cognition-walkthrough.md)。

## 结构变化应覆盖的真实缺陷

| 已知问题 | 负责修复的位置 | 必须迁移的消费者 |
|---|---|---|
| replace/archive/restore/remember 被 expected 带偏 | 文稿目标解析与事务核心 | CLI、SDK、草稿采用/编辑调用 |
| Namespace 改名后旧句柄误归属 | Namespace owner 与句柄结构 | records/concepts/retrieval/annotations/feedback |
| 来源越界、引用形状改变依据失效 | content 范围与 cognition 来源解析 | 手写、写作、查询选段、草稿、反馈 |
| historical 当前标注不刷新、已存在标注不再激活 | Annotation 活动指针事务 | 人工标注、后台标注、current 捕获 |
| rewrite 丢引用定义、定义行不可编辑 | 无损文本准备与 TaskDefinition | 模型预览、真实请求、编辑解释 |
| 未知时间域生成后失败丢结果 | 写作准备与生成后恢复边界 | journalize、reflect、draft adoption |
| 配置读取重复全量校验 | ConfigSnapshot | 所有配置读取者及任务冻结 |
| 配置变更绕开执行、老 worker 丢 Trace | 应用用例/Execution | SDK、CLI、DBOS 回到业务循环的工作 |
| 结构缓存阻塞、重复取消泄漏子进程 | content/cache、Execution、AudioVisual | 模型/媒体/Wave 与 shutdown |
| 查询反馈四项读取五份完整快照 | Wave 描述/批量候选 | FeedbackOperations 与查询展开 |

## 完成条件

用户可以从新实例完成保存、后台维护、关闭重开、继续执行和检索。错误目标无修改；名称复用不改变句柄归属；来源选区和正式版本不会在通路中丢失；全文改写发送原文；活动标注改变立即使 current 投影过期；公开入口具有相同执行/诊断语义；取消完成后资源已退出。

事务回滚不留下 Job，已提交文稿附带明确的维护引用。已记录的成功计算在恢复采用时不重发模型；从 ResultStore 的稳定业务键恢复已持久接受但尚未 checkpoint 的成功语言响应；业务提交后的步骤重放不重复创建认知或覆盖新 head。标注失败时词项检索仍可获得正文。短进程退出仅停止 worker 宿主，不擅自把排队/中断工作标记为用户取消；wait 不占普通执行配额。

检查一次完整的失败管理路径：保存文稿 → 标注的下游采用阶段失败为 ERROR → 从该失败步骤产生 fork 工作（保留原 queued policy） → 复用已保存的语言响应 → 完成正式采用与后续刷新。与崩溃后自动续跑、取消后 resume 是三种真实行为，CLI 和 SDK 的状态/引用须区分。

没有第二套模型/媒体/来源/任务实现；旧内部接口和仅服务旧实现的测试/脚手架同步删除。验证按 [完整通路](../development/durable-cognition-verification.md)执行。禁止将 Windows、真实模型质量或进程强杀未实际验证的部分写成已验证。

## 后续范围

WorkContext、Active Memory、Steward 与情境反馈是下一条认知使用链；Episode/Observation、五轴历史、Ego/Social/Motivation 随后推进，见 [后续发展](cognitive-use-next.md)。本轮不实现通用事件总线、远程服务协议、自动反思平台、任意历史视图或 Native 增量算法。
