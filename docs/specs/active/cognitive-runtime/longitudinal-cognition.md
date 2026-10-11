# 纵向认知

[返回当前产品合同](../../INDEX.md)

## Owner 与路径

Session-bound Observation → Runtime ExperienceItem → automatic Episode → Memory-owned Journal → host-granted Maintenance → Memory/CognitiveSchema consolidation。

Runtime 持有 CognitiveClock、Experience feed、segmentation cursor/draft 与 MaintenanceNeed。Material 提交 Observation；Memory 持有 Episode、Journal、Memory 与 CognitiveSchema Authority。Core 执行模型和有界维护机会；Kernel 组合 owner；Retrieval 提供可重建 Serving。

## 时间与经验捕获

正常运行使用 SystemCognitiveClock；内部测试和研究可注入按 Subject 单调推进的 ManualCognitiveClock。HTTP/DB timeout、重试和 worker lease 使用基础设施时间。公开 RPC 不提供 clock mutation。Core 通过 private `KernelQueryService.GetCognitiveTime` 读取 Subject clock；NousQL 的相对时间窗口也使用该认知时间。

Observation `observed_at` 可省略，缺省使用 CognitiveClock。发生时间保留调用方提供的已知或 unknown 语义。Subject、Cognitive Seed、Material 来源、Memory、CognitiveSchema、Episode、Journal 及语义 lifecycle/UseEvent 的 timestamps 由 owner 提交分配；工作流重放保留原 formation time。Memory、CognitiveSchema、Episode、Journal 的创建和修订输入不接受 caller `formed_at`；Schema 使用 `CognitiveSchemaContent` 输入。

Session-bound Observation 在 Material 提交事务内写入一个 ExperienceItem，复用现有 invalidation 返回的 Subject `authority_seq` 作为 `recorded_seq`。记录包含 Session、foreground WorkContext ID/revision 和边界上下文，并 upsert 到期的 `episode_segment` need。无 Session 的 Observation 保持普通 Material/Evidence，不进入自动分段。摄入过程中不调用模型。

## 初始 Episode 与语义 refinement

初始自动 track 为 `interaction`。Runtime 按 recorded sequence 推进 durable cursor/draft；Session 变化本身保持连续。hard idle、结束的 WorkContext、多个上下文切换，或 soft idle 伴随上下文/Session 切换可关闭 draft。无新观察时，到期维护负责 idle closure。ready draft 使用稳定 operation identity 经 Memory 提交；ack 验证对应 commit receipt 后删除 draft 和成员，cursor 不保留已删除 draft 的引用。Memory commit 后、ack 前崩溃时，ready draft 和稳定 operation identity 重放得到原 Episode，再完成 ack。迟到观察排入有界局部 resegmentation。

`recorded_seq` 决定 exactly-once processing、durable cursor 和摄入顺序；experience chronology 使用 `(observed_at, recorded_seq)`。draft 最终成员、局部 repair、模型 partition input 与提交的 Episode 都按 experience chronology 排列，同 observed time 用 recorded sequence 确定顺序。

历史 repair 以迟到 Observation 的 observed time 定位同 parent/track 的 overlapping current Episodes 和两侧最近邻，构建局部 partition 后加入迟到 occurrence。`episode.max_neighbor_span_seconds` 限制受影响 neighborhood 的 experience span；经历距今天的年龄不限制 repair。neighborhood 的 Episode 数、experience span 或 materialization safety budget 超界时进入 `historical_repair_scope_exceeded` blocked。

`episode_segmentation` 接收精确、有序 member keys 和当前 Episode 组织，返回 `no_change` 或完整 partition。Memory 原子应用 N-to-M partition：1-to-1 修订同一对象；split/merge/general partition 创建替代对象并 withdraw sources，保留 exact lineage。缺失、重复、重排或虚构成员使整次提交失败。

## Maintenance 与工作流

`RuntimeService` 承载 canonical Session、WorkContext、Observation 和 Use 操作；Core 的 `CognitionService` 提供公开维护 grant 与查询/投影编排。Core 通过 private `KernelMaintenanceService` 调用 durable need 和 longitudinal owner commit，通过 `KernelModelWorkflowService` 管理模型执行重放状态。

MaintenanceNeed 在 active scope 内合并 trigger，due time 使用 CognitiveClock。并发 claim 使用数据库 row locking；基础设施 lease 与 token fence 控制 worker，过期 lease 可重新 claim。finish 保留执行期间新增的 trigger。

公开 `GrantMaintenance` 接收 `max_operations`、`max_model_calls`、`max_elapsed_ms`。Core standalone loop 完整使用 active Subject pagination，保存跨 poll tick 的 continuation；每个 Subject 每次获得至多一个操作机会，耗尽 tick budget 后从停止处继续，列表尾部 wrap。删除、停用或失效 page token 会跳过或重新建立 cursor。Core restart 可以从稳定列表起点开始。全局 operation/model/elapsed budgets 限制每次 tick；Kernel 没有自主认知 cron，只持有 durable needs、claim、lease 与 owner semantics。

Grant response 的 disposition 明确区分 `disabled_by_policy`、`no_eligible_work`、`processed` 与 `opportunity_exhausted`。Host 机会耗尽或取消时，尚未完成的 need 返回 pending/deferred，保存已提交 proposal 与进度，不把机会预算当成 owner invariant failure 或 provider retry exhaustion；下一次明确 grant 继续 durable work。Provider 自身超时仍按 infrastructure retry 处理。官方 Client 默认 deadline 覆盖请求机会及回应余量，显式 caller deadline 优先。

显式机会 elapsed 上限为 900000ms，standalone tick 可配置同一上限，默认仍 60000ms。`core_execution.opportunity` 将普通 model/resource 工作期限、取消后的 cleanup、need acknowledgement 和 response margin 分开命名；Core 与官方 Client 使用同一 typed policy。Client 从 active Configuration 读取实际 policy，按显式 grant 的 work budget 加 cleanup/ack/margin 派生响应等待；普通 formation、derivation、embedding preparation 和 selected resource materialization 使用 policy 的 work budget。显式较短 caller deadline 或 signal 仍优先。MCP 在进程内复用 Client，没有独立等待秒数；外部 Host tool timeout 应覆盖该机会。每条 model route 继续使用自己的 timeout 和调用计数，输入不适配的 route 在调用计数前跳过。

Maintenance claim 的 infrastructure lease 从相同 work、cleanup 和 acknowledgement 预算派生，并至少覆盖 Runtime worker lease。Core 向 private ModelWorkflow reservation 显式传入该 lease；普通 workflow 也使用其实际执行机会，删除另一处 `model_workflow.lease_seconds` 默认值。Persistence 验证真实父 claim/token 后使 child reservation 覆盖实际父期限，不延长父授权；owner mutations 继续执行维护 claim 及内容/epoch fences。取消或 work budget 到期后，保存 attempt telemetry 和 release 共用有界 cleanup 截止时间，随后 finishMaintenance 使用独立 acknowledgement 预算；原 work signal 不会提前取消这两阶段。

维护种类为 `episode_segment`、`episode_resegment`、`journal_review`、`journal_revalidate`、`memory_consolidate`。Core 根据当前可执行角色构造 claim 的 allowed kinds：resegment 需要 `episode_segmentation`，Journal review/revalidate 需要 `journal_synthesis`，consolidation 需要 `memory_consolidation`；初始 segmentation 无需模型。未就绪角色的 needs 保持 durable、无 worker lease、attempt count 不增长。角色配置按当前 Core model runtime 的 restart 生效合同处理。

MaintenanceNeed 状态为 `pending / leased / blocked / satisfied / obsolete`。deferred 表示时间推进或已知未来事件能够改变条件，保留带 next due 的 pending work。blocked 表示依赖、配置或输入需要实际改变，不参与普通 due-time lease。来源 revision/lifecycle、显式 maintenance refresh 或有效配置快照变化可重新激活 blocked need；retry exhaustion 还可由当前 executable model configuration digest 的变化唤醒。真实 provider/network 故障使用配置的指数退避，等待基础设施时间；连续失败达到 retry attempt 上限后 blocked。新 trigger 重置 retry 状态，每个 need 的 trigger revision 随真实 trigger 或配置唤醒推进；执行期间的新 trigger revision 保留为 pending work。

unexpected scheduler/runtime failures 输出 Subject、stage、已知 need kind、稳定 problem class 和 retry/blocked decision；相同 scheduler failures 按重复次数聚合。凭据、配置值和原始 model payload 不进入诊断。正常 blocked 或未配置角色无需 exception 日志。

Episode/Journal/Memory/Schema revisions 是 durable cognition history；Observation/ExperienceItem 是 durable experience；EpisodeDraft、MaintenanceNeed 和 model workflow 是 temporary runtime work state。terminal need 按 `maintenance.terminal_retention_seconds` 保留短期 finish replay，再在 claim/ack 路径删除；pending、leased、blocked 不被 terminal housekeeping 删除。

Core 的 `model_workflow_operations` 保存固定 model/plan snapshot、proposal 与 outcome。operation identity 由 need、trigger authority sequence 和 trigger revision 生成，proposal 在 owner commit 前持久化；重试复用已存 proposal/outcome。maintenance workflow 显式绑定 need 和 trigger；ack 成功后删除已完成的关联 workflow。新 trigger 已替代的旧 workflow 在执行 lease 释放/过期后回收。显式 Memory formation、Material workflow 和仍需 retry 的当前 workflow 保持自身合同。owner 验证 Subject、head、epoch、lifecycle、来源 catalog 和 authority sequence；变化返回 stale 并刷新维护计划。

Workflow owner 是 1..64 字节的 semantic identifier，格式为 `[a-z][a-z0-9_]*`；Persistence 不枚举认知领域。private Kernel API 仅接受已实现的 `memory / material` 调用路径；Longitudinal 的 Memory-domain operations 使用 `memory`。

## Journal

JournalId 是稳定对象，JournalRevisionId 是不可变 exact narrative。每个 point 有 role、text 和至少一个精确 support；bounded narrative 与 points 对应。`journal_synthesis` 使用 settled EpisodeRevision sources 和 supplied basis keys，模型不能创造来源引用。

创建、修订、get/exact revision、history/list、suppress/restore、withdraw/reaccept 与 purge 均归 Memory。source revision/lifecycle/purge 变化使当前 Journal `revalidation_required`，排入 `journal_revalidate`；重建提交新的 revision，scope 消失可 withdraw。Journal provenance 展开原来源 lineage，Journal 不增加独立 evidence root。

Journal 重验证等待全部当前来源稳定后再规划完整集合。来源数量或时间跨度超界时，Journal 保持 `revalidation_required`，need 分别进入 `journal_scope_bound_exceeded` 或 `journal_scope_span_exceeded` blocked；时间推进不重新运行。相关来源/Journal 变化、配置 bound 变化或显式 refresh 可以唤醒。重验证覆盖声明的完整 source scope；新 Journal review 可按边界选择已稳定的前缀。

## Consolidation

整合与 Journal synthesis 可读取手工 Episode 的 Occurrence 或 cognition members；普通 Occurrence 的目录条目保留正文和观察时间，Session 与 experience recorded sequence 仅在实际存在时提供。自动 resegmentation 使用 Session ExperienceItem 的完整顺序。

`memory_consolidation` 使用 `MemoryConsolidationText`，scope 为 current eligible EpisodeRevision 或 JournalRevision。计划包含有界 source/member/support/entity catalogs、independent roots 和通过 Query/Serving 选出的最多 16 个当前 Memory/Schema context candidates。候选包括 exact revision/epoch、正文及 text_truncated 标记、Schema applicability/boundary/tags、独立时间轴和按 use kind 汇总的 meaningful use；presented 不计入摘要。截断标记描述模型当前可见范围，不表示来源或已存 cognition 缺少该内容，不能仅据此修订候选。

proposal action 为 skip、create/revise Memory、create/revise CognitiveSchema、link relation。引用只能选择 supplied catalog 或较早 action result。Core 保存一次模型 proposal 后逐项调用 canonical FormMemory、ReviseMemory、Create/ReviseCognitiveSchema 和 LinkRevisions；每项使用由 workflow id、index 与 action kind 派生的稳定 operation ID。各 owner 的 receipt 与 Authority mutation 同事务；后项非法不回滚此前独立提交，依赖失败 action 的 relation 记录 skipped_dependency，其他独立 action 继续。逐项结果保存到 workflow，transport retry 重放同一 proposal 和 ID，不再次调用模型。目标 epoch 已陈旧时记录 stale；owner invariant/internal failure 停止 grant 并暴露工程错误。Planner 为每个 revision candidate 提供排除该对象全部旧 revision 的 eligible basis keys；owner 继续执行 cycle、provenance、lifecycle 与身份核验。skip 不修改认知 Authority。Journal 是可选来源，Episode 可直接整合。

Query/Serving、WorkContext、UseEvent 和下游失效合同分别见 [Query](../memory-reference-profile/03-query-serving.md)、[WorkContext](work-context.md)、[Use](../memory-reference-profile/02-runtime-use.md) 和 [Authority](../memory-reference-profile/01-memory-authority-provenance.md)。


## 独立 concept maintenance

`concept_maintenance` 复用 durable need、host grant、lease、retry、固定 ModelWorkflow snapshot 和模型调用/elapsed 预算。accepted/revised Memory、Episode、Journal、CognitiveSchema 在 owner mutation 同一事务排入 exact focus；meaningful use 跨过 `maintenance.concept_use_review_interval` 时产生 review，普通 presented 不触发。

Memory planner 只围绕一个 current eligible exact cognition。局部输入包含 focus 语义、来源、aboutness Entity、已附 Tags、名称/别名匹配、既有 embedding material 的相似候选和一跳 AssociationEvidence；不拼整个 Subject cognition catalog，不调用 embedding provider。固定 snapshot 保存 typed canonical refs、TagRevisionTarget、AssociationSupport 和当前配置 digest；模型只看到 local keys 与有界描述。

`concept_maintenance` role 使用 `prompts/memory/concept-maintenance.md` 和独立 Structured Contract，最多四个 ordered suggestions，配置可以进一步收紧。Core 保存 proposal 后逐项调用 Create/Revise/Merge/SplitTag、Create/RevokeAssociation；operation id 来自 workflow id、index 和 kind。每项保存 committed/no_change/rejected_invalid/stale/skipped_dependency 与实际结果，临时 `new_` keys 解析为先前成功返回的 Tag identity。后续失败保留先前成功，依赖失败只跳过依赖项，独立项继续；transport 重试复用已存 proposal 和相同 owner receipts，不重新调用模型。owner invariant 停止该 grant，返回 internal_failure。

Tag merge/split 保留 exact supports 和 lineage，一个明确来源可以支持 alias/equivalence 或语义分化；不要求两个独立根。merge/split 自身仍为天然原子操作。后置 `tag_attachment` 是正向 exact cognition→Tag AssociationEvidence，不修改旧 cognition revision。owner 校验 Subject、当前 endpoint 生命周期、relation registry、方向/极性、显式支持和 producer，不递归证明整个 cognition graph。Serving 直接使用既有 Memory dependencies、Episode members、Journal sources、Schema evidence 作为结构 adjacency；不要求复制 AssociationEvidence。contradiction/negative evidence 保留独立语义。

Accretion 是按 center 按需计算的派生信号：distinct roots、current members、Episode recurrence、observed span、association degree/diversity、meaningful use、counterevidence、可选 cached coherence 和 genericity。没有持久化 Subject-wide cache、global confidence 或 usefulness truth。`maintenance.accretion` 只暴露 enabled、generic_degree、recurrence_review；关闭后基础 concept maintenance 仍工作。review priority 与 merge/split hints 在 planner 即时计算；member overlap/coherence 阈值为实现常量。普通 presented 不增加支持，meaningful use 不改变 epistemic class 或独立根。

Consolidation 的候选查找使用独立 text-only lookup：输入是引用的 source 原文，不作为待闭合的用户意图。实体目录提供 source 文本中出现的 display name/alias 的有界候选，加上已有候选的 aboutness；目录匹配本身不写入 aboutness，仍由模型选择、owner 校验。形成阶段需要 lexical Serving 为 continuing claim 提供当前候选，dense/topology 与付费 embedding 在检索验证前保持关闭。

Directory 的当前有效 Entity binding 是 Subject 内可引用身份，即使尚未被 Memory aboutness 或 observation actor 使用。通用 reference 校验承认该 binding；不把它自动转换成 aboutness。跨 Subject 和 tombstoned binding 仍不能仅凭目录获得可引用资格。
