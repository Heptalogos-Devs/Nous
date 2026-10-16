# 纵向认知

[返回当前产品合同](../../INDEX.md)

## Owner 与路径

Session-bound Observation → Runtime ExperienceItem → automatic Episode → Memory-owned Journal → host-granted Maintenance → Memory/CognitiveSchema consolidation。

Runtime 持有 CognitiveClock、Experience feed、segmentation cursor/draft 与 MaintenanceNeed。Material 提交 Observation；Memory 持有 Episode、Journal、Memory 与 CognitiveSchema Authority。Core 执行模型和有界维护机会；Kernel 组合 owner；Retrieval 提供可重建 Serving。

## 时间与经验捕获

时间与历史切面统一由[时钟合同](clock.md)拥有。Nous 复用正式 CognitiveClockService；现实 observed/recorded、网络/数据库 timeout 与 retry/lease 不受世界变速影响。形成和实际提交的世界语境分别保留，工作流恢复沿用原成果形成时间。Memory/Schema/Episode/Journal 的输入不接受任意 caller 形成时间。

只有实际释放给 Subject 的 subject_experience 进入自动 ExperienceItem/反思路径。受信 WorldLife 的角色可见 world_realized 是合法虚拟经历；world author/review 草稿、未释放 worker 结果和隐藏 NPC 状态作为 execution_only 保存在其所属能力，不进入此 feed。该性质由正式 Host/角色绑定提供，不能由普通模型字符串选择。

Session-bound 接触在同一 Material 事务写入 ExperienceItem，保留 Session、WorkContext、正式接触来源与时间域，并产生现有 episode_segment need。重复交接使用原请求身份与 receipt，不重复写经验或再次执行工具。摄入事务不调用模型。无 Session 的普通 Material 不自动分段；内部世界活动通过正式非 conversation 运行取得适用的 Subject Session，不发明 QQ 会话。

real recorded sequence 决定摄入与 exactly-once 次序；生活组织使用接触所属域与角色可见世界见闻点，保留实际现实 observed/recorded。晚实现虚拟见闻可触发同域有界 repair，但不能穿越显式现实 as-of 或改写已经确定的角色知情/Claim。不同域无 mapping 时不按裸数字混合排序、邻域或聚合。experience_items 同时保留实际 observed_at 与 experience_at/domain；draft、初始 Episode、partition 和 repair 使用同域经历时点，真实记录次序负责 exactly-once。

## 初始 Episode 与语义 refinement

初始自动 track 为 `interaction`。Runtime 按 recorded sequence 推进 durable cursor/draft；Session 变化本身保持连续。hard idle、结束的 WorkContext、多个上下文切换，或 soft idle 伴随上下文/Session 切换可关闭 draft。无新观察时，到期维护负责 idle closure。ready draft 使用稳定 operation identity 经 Memory 提交；ack 验证对应 commit receipt 后删除 draft 和成员，cursor 不保留已删除 draft 的引用。Memory commit 后、ack 前崩溃时，ready draft 和稳定 operation identity 重放得到原 Episode，再完成 ack。迟到观察排入有界局部 resegmentation。

`recorded_seq` 决定 exactly-once processing、durable cursor 和摄入顺序；experience chronology 在同一适用域内使用正式接触时点与 recorded sequence。draft 最终成员、局部 repair、模型 partition input 与提交的 Episode 都按 experience chronology 排列，同 observed time 用 recorded sequence 确定顺序。

历史 repair 以迟到 Observation 的 observed time 定位同 parent/track 的 overlapping current Episodes 和两侧最近邻，构建局部 partition 后加入迟到 occurrence。`episode.max_neighbor_span_seconds` 限制受影响 neighborhood 的 experience span；经历距今天的年龄不限制 repair。neighborhood 的 Episode 数、experience span 或 materialization safety budget 超界时进入 `historical_repair_scope_exceeded` blocked。

`episode_segmentation` 接收精确、有序 member keys 和当前 Episode 组织，返回 `no_change` 或完整 partition。Memory 原子应用 N-to-M partition：1-to-1 修订同一对象；split/merge/general partition 创建替代对象并 withdraw sources，保留 exact lineage。缺失、重复、重排或虚构成员使整次提交失败。

## Maintenance 与工作流

`RuntimeService` 承载 canonical Session、WorkContext、Observation 和 Use 操作；Core 的 `CognitionService` 提供公开维护 grant 与查询/投影编排。Core 通过 private `KernelMaintenanceService` 调用 durable need 和 longitudinal owner commit，通过 `KernelModelResultsService` 保存和复用实际模型结果。

MaintenanceNeed 在 active scope 内合并 trigger；due_at 是现实唤醒时间，认知 idle/settle 使用同域语义接触/提交点判断。暂停、变速或恢复后由正式 Clock 操作唤醒既有义务重评，不授执行/模型预算，也不清网络重试限制。单一 Core 按 Subject 串行执行 Host grant；Runtime 从 pending needs 选择固定 trigger snapshot，执行期间记录仍为 pending。每次新 trigger 推进 trigger revision；ack 只对匹配 captured authority sequence/revision 的 pending need 应用 disposition，因此新工作不丢失。Core crash 后工作立即可重新选择，不需要租约过期或 takeover。

公开 `GrantMaintenance` 接收 `max_operations`、`max_model_calls`、`max_elapsed_ms`。Core standalone loop 完整使用 active Subject pagination，保存跨 poll tick 的 continuation；每个 Subject 每次获得至多一个操作机会，耗尽 tick budget 后从停止处继续，列表尾部 wrap。删除、停用或失效 page token 会跳过或重新建立 cursor。Core restart 可以从稳定列表起点开始。全局 operation/model/elapsed budgets 限制每次 tick；Kernel 没有自主认知 cron，只持有 durable needs、trigger-fenced acknowledgement 与 owner semantics。

Grant response 的 disposition 明确区分 `disabled_by_policy`、`no_eligible_work`、`processed` 与 `opportunity_exhausted`。Host 机会耗尽或取消时，尚未完成的 need 返回 pending/deferred，保存已提交 proposal 与进度，不把机会预算当成 owner invariant failure 或 provider retry exhaustion；下一次明确 grant 继续 durable work。Provider 自身超时仍按 infrastructure retry 处理。官方 Client 默认 deadline 覆盖请求机会及回应余量，显式 caller deadline 优先。

显式机会 elapsed 上限为 900000ms，standalone tick 可配置同一上限，默认仍 60000ms。`core_execution.opportunity` 将普通 model/resource 工作期限、取消后的 cleanup、need acknowledgement 和 response margin 分开命名；Core 与官方 Client 使用同一 typed policy。Client 从 active Configuration 读取实际 policy，按显式 grant 的 work budget 加 cleanup/ack/margin 派生响应等待；普通 formation、derivation、embedding preparation 和 selected resource materialization 使用 policy 的 work budget。显式较短 caller deadline 或 signal 仍优先。MCP 在进程内复用 Client，没有独立等待秒数；外部 Host tool timeout 应覆盖该机会。每条 model route 继续使用自己的 timeout 和调用计数，输入不适配的 route 在调用计数前跳过。

Vault 的 Maintenance Lease 表示 Host 授予的时间、资源、模型服务、网络权限和取消边界；该授权由本节的 bounded Host grant 实现，不要求持久化 worker token 或运行中租约。

Host grant 的 work、model-call 和 operation budgets 在 Core 全部 Subject 排队与实际执行中持续生效。Private operation start 验证当前 pending MaintenanceTrigger；没有 parent/child lease、运行中状态、busy token 或 deadline。领域提交继续执行来源、内容、revision 和 epoch fences。取消后 telemetry/proposal/outcome save 使用有界 cleanup 时间，finishMaintenance 使用独立 acknowledgement 预算；取消不授权继续认知 mutation。

维护种类为 `episode_segment`、`episode_resegment`、`journal_review`、`journal_revalidate`、`memory_consolidate`。Core 根据当前可执行角色构造 selection 的 allowed kinds：resegment 需要 `episode_segmentation`，Journal review/revalidate 需要 `journal_synthesis`，consolidation 需要 `memory_consolidation`；初始 segmentation 无需模型。未就绪角色的 needs 保持 pending，不执行模型。角色配置按当前 Core model runtime 的 restart 生效合同处理。

MaintenanceNeed 状态为 `pending / blocked / satisfied / obsolete`。deferred 表示时间推进或已知未来事件能够改变条件，保留带 next due 的 pending work。blocked 表示依赖、配置或输入需要实际改变，不参与普通 due-time selection。来源 revision/lifecycle、显式 maintenance refresh 或有效配置快照变化可重新激活 blocked need；retry exhaustion 与 capability unavailable 还可由当前 executable model configuration digest 的变化唤醒。补齐凭据并重启后，下一次显式 grant 可重新选择此前因角色不可用而 blocked 的 need，无须修改配置文档。真实 provider/network 故障使用配置的指数退避，等待基础设施时间；连续失败达到 retry attempt 上限后 blocked。新 trigger 重置 retry 状态，每个 need 的 trigger revision 随真实 trigger 或配置唤醒推进；执行期间的新 trigger revision 保留为 pending work。

unexpected scheduler/runtime failures 输出 Subject、stage、已知 need kind、稳定 problem class 和 retry/blocked decision；相同 scheduler failures 按重复次数聚合。凭据、配置值和原始 model payload 不进入诊断。正常 blocked 或未配置角色无需 exception 日志。

Episode/Journal/Memory/Schema revisions 是 durable cognition history；Observation/ExperienceItem 是 durable experience；EpisodeDraft 与 MaintenanceNeed 是临时 Runtime 工作；model_results 只保存实际提案、结果与调用记录。terminal need 按 `maintenance.terminal_retention_seconds` 保留短期 inspection，再在 selection/ack 路径删除；pending、blocked 不被 terminal housekeeping 删除。

一次 grant 按 priority 降序、due 和 need ID 的既有顺序有界翻页；游标使用选取时的位置，已查看 ID 只保留在本次调用。未获得生成机会的项不消耗操作预算，也不确认 pending、重置 retry 或重写 due；随后继续查看已有成果和确定性操作。每个实际处理的 need（含终结或失败尝试）消耗一个操作名额，公开响应返回 `operationsConsumed`，scheduler 直接使用它。模型预算只计实际 provider attempt，跳过 route 不计。执行期限覆盖整次机会。scheduler 的 grant RPC 失败时停止本轮，不猜测未知的操作消耗；下一轮继续已推进的 Subject 顺序。required 角色不可用或执行失败保留正式 `ErrorDetail` 的 capability、requirement 与恢复建议，optional 的有效降级结果由功能 owner 决定。

Core 的 `model_results` 在实际生成后保存 proposal、必要的 owner 输入、依赖与 telemetry，提交后保存 outcome；执行前不登记 snapshot。identity 由 need、trigger authority sequence 和 trigger revision 生成。后续显式机会复用已存 proposal/outcome；只有重新生成才检查角色可用性和模型调用预算。确定性 Journal 撤回不创建模型记录。多 action 提案按原顺序调用 owner，稳定 child operation ID 和 owner receipt 重放已提交结果，不持久化逐项 progress、临时绑定或第二份回执。后项失败不回滚已提交的独立前项。结果关联 need/trigger，ack 删除已完成记录，selection/ack 清理被新 trigger 替代的记录。显式形成、Material 结果和仍需提交的当前提案保留其结果合同。owner 验证 Subject、head、epoch、lifecycle、来源和 authority sequence；变化返回 stale 并刷新维护计划。取消与机会耗尽保留 need 与实际已存成果；现实唤醒点不改变认知条件本身的域或真假。

Workflow owner 是 1..64 字节的 semantic identifier，格式为 `[a-z][a-z0-9_]*`；Persistence 不枚举认知领域。private Kernel API 仅接受已实现的 `memory / material` 调用路径；Longitudinal 的 Memory-domain operations 使用 `memory`。

## Journal

JournalId 是稳定对象，JournalRevisionId 是不可变 exact narrative。每个 point 有 role、text 和至少一个精确 support；bounded narrative 与 points 对应。`journal_synthesis` 使用 settled EpisodeRevision sources 和 supplied basis keys，模型不能创造来源引用。

创建、修订、get/exact revision、history/list、suppress/restore、withdraw/reaccept 与 purge 均归 Memory。source revision/lifecycle/purge 变化使当前 Journal `revalidation_required`，排入 `journal_revalidate`；重建提交新的 revision，scope 消失可 withdraw。Journal provenance 展开原来源 lineage，Journal 不增加独立 evidence root。

Journal 重验证等待全部当前来源稳定后再规划完整集合。来源数量或时间跨度超界时，Journal 保持 `revalidation_required`，need 分别进入 `journal_scope_bound_exceeded` 或 `journal_scope_span_exceeded` blocked；时间推进不重新运行。相关来源/Journal 变化、配置 bound 变化或显式 refresh 可以唤醒。重验证覆盖声明的完整 source scope；新 Journal review 可按边界选择已稳定的前缀。

## Consolidation

整合与 Journal synthesis 可读取手工 Episode 的 Occurrence 或 cognition members；普通 Occurrence 的目录条目保留正文和观察时间，Session 与 experience recorded sequence 仅在实际存在时提供。自动 resegmentation 使用 Session ExperienceItem 的完整顺序。

`memory_consolidation` 使用 `MemoryConsolidationText`，scope 为 current eligible EpisodeRevision 或 JournalRevision。计划包含有界 source/member/support/entity catalogs、independent roots 和通过 Query/Serving 选出的最多 16 个当前 Memory/Schema context candidates。候选包括 exact revision/epoch、正文及 text_truncated 标记、Schema applicability/boundary/tags、独立时间轴和按 use kind 汇总的 meaningful use；presented 不计入摘要。截断标记描述模型当前可见范围，不表示来源或已存 cognition 缺少该内容，不能仅据此修订候选。

整合 delay 从实际 source Authority commit 的 semantic stamp 计算，同域成熟后才准备整合正文。Episode/Journal 的初次现实唤醒按当前 clock mapping 投影；暂停、缺 mapping 或尚未成熟的 scope 返回 deferred 和现实 next_due。旧故事发生时间不使刚提交的认知提前成熟。

proposal action 为 skip、create/revise Memory、create/revise CognitiveSchema、link relation。引用只能选择 supplied catalog 或较早 action result。Core 保存一次模型 proposal 后逐项调用 canonical FormMemory、ReviseMemory、Create/ReviseCognitiveSchema 和 LinkRevisions；每项使用由 operation id、index 与 action kind 派生的稳定 operation ID。各 owner 的 receipt 与 Authority mutation 同事务；后项非法不回滚此前独立提交，依赖失败 action 的 relation 记录 skipped_dependency，其他独立 action 继续。本次逐项结果随最终 outcome 一次保存，transport retry 重放同一 proposal 和 ID，不再次调用模型。目标 epoch 已陈旧时记录 stale；owner invariant/internal failure 停止 grant 并暴露工程错误。Planner 为每个 revision candidate 提供排除该对象全部旧 revision 的 eligible basis keys；owner 继续执行 cycle、provenance、lifecycle 与身份核验。skip 不修改认知 Authority。Journal 是可选来源，Episode 可直接整合。

Query/Serving、WorkContext、UseEvent 和下游失效合同分别见 [Query](../memory-reference-profile/03-query-serving.md)、[WorkContext](work-context.md)、[Use](../memory-reference-profile/02-runtime-use.md) 和 [Authority](../memory-reference-profile/01-memory-authority-provenance.md)。

## 独立 concept maintenance

`concept_maintenance` 复用 durable need、host grant、retry、已保存的实际模型结果 和模型调用/elapsed 预算。accepted/revised Memory、Episode、Journal、CognitiveSchema 在 owner mutation 同一事务排入 exact focus；meaningful use 跨过 `maintenance.concept_use_review_interval` 时产生 review，普通 presented 不触发。

Memory planner 只围绕一个 current eligible exact cognition。局部输入包含 focus 语义、来源、aboutness Entity、已附 Tags、名称/别名匹配、既有 embedding material 的相似候选和一跳 AssociationEvidence；不拼整个 Subject cognition catalog，不调用 embedding provider。owner plan 提供 typed canonical refs、TagRevisionTarget、AssociationSupport 和策略；实际产生提案后保存提交所需输入，模型只看到 local keys 与有界描述。

`concept_maintenance` role 使用 `prompts/memory/concept-maintenance.md` 和独立 Structured Contract，最多四个 ordered suggestions，配置可以进一步收紧。Core 保存 proposal 后逐项调用 Create/Revise/Merge/SplitTag、Create/RevokeAssociation；operation id 来自 operation id、index 和 kind。每项保存 committed/no_change/rejected_invalid/stale/skipped_dependency 与实际结果，临时 `new_` keys 解析为先前成功返回的 Tag identity。后续失败保留先前成功，依赖失败只跳过依赖项，独立项继续；transport 重试复用已存 proposal 和相同 owner receipts，不重新调用模型。owner invariant 停止该 grant，返回 internal_failure。

Tag merge/split 保留 exact supports 和 lineage，一个明确来源可以支持 alias/equivalence 或语义分化；不要求两个独立根。merge/split 自身仍为天然原子操作。后置 `tag_attachment` 是正向 exact cognition→Tag AssociationEvidence，不修改旧 cognition revision。owner 校验 Subject、当前 endpoint 生命周期、relation registry、方向/极性、显式支持和 producer，不递归证明整个 cognition graph。Serving 直接使用既有 Memory dependencies、Episode members、Journal sources、Schema evidence 作为结构 adjacency；不要求复制 AssociationEvidence。contradiction/negative evidence 保留独立语义。

Accretion 是按 center 按需计算的派生信号：distinct roots、current members、Episode recurrence、observed span、association degree/diversity、meaningful use、counterevidence、可选 cached coherence 和 genericity。没有持久化 Subject-wide cache、global confidence 或 usefulness truth。`maintenance.accretion` 提供 enabled、generic_degree、recurrence_review、member_budget（默认 32，最多 256）和 coherence_threshold（默认 0.35）；关闭后基础 concept maintenance 仍工作。review priority 与 merge/split hints 在 planner 即时计算；成员按 kind/value 的稳定顺序读取，内容、direct support、递归认知依据与材料证据定位使用同一读取事务，遍历直接复用已取得的 support；coherence 使用该顺序的首项为基准，member overlap 保留现有规则。普通 presented 不增加支持，meaningful use 不改变 epistemic class 或独立根。

Consolidation 的候选查找使用独立 text-only lookup：输入是引用的 source 原文，不作为待闭合的用户意图。实体目录提供 source 文本中出现的 display name/alias 的有界候选，加上已有候选的 aboutness；目录匹配本身不写入 aboutness，仍由模型选择、owner 校验。形成阶段需要 lexical Serving 为 continuing claim 提供当前候选，dense/topology 与付费 embedding 在检索验证前保持关闭。

Directory 的当前有效 Entity binding 是 Subject 内可引用身份，即使尚未被 Memory aboutness 或 observation actor 使用。通用 reference 校验承认该 binding；不把它自动转换成 aboutness。跨 Subject 和 tombstoned binding 仍不能仅凭目录获得可引用资格。
