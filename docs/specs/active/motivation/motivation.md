# Motivation、期望条件与评价

[返回合同目录](../../INDEX.md)

## Authority 与消费

`crates/motivation` 拥有 Desire、Concern 和 DesiredCondition，迁移为 `0010_motivation.sql`；`0012_motivation_source_read.sql` 在正式 `0011_source_read.sql` 后扩展唯一固定 dispatcher，不改写已发布迁移。`motivation` 是独立可选 Subject capability；既有 Subject 默认关闭，Memory-only 不依赖它。愿望允许尚未形式化，关切表示持续关注，期望条件才保存可评价的命题。采用/撤回状态与评价结果分别维护。Heptalogos 的 Pursuit、活动、工具范围、计划和外部承诺不进入此 owner。

对象和不可变 revision 有正式独立引用，采用、精确 head/epoch 修订、追加证据、历史分页、生命周期和 purge 复用 MutationEnvelope、RevisionBasis、producer、来源闭包与历史传播屏障。matterKey 是稳定事项身份；scope、陈述和条件可以修订。细化、前置、冲突、替代、支持和长期优先关系引用精确 Motivation revision，不表示当前调度优先级。

Canonical `MotivationService` 与官方 `client.motivation` 提供 adopt/revise/addEvidence/get/getRevision/list/listRevisions/select/setLifecycle/purge/evaluate/getEvaluation/listEvaluations。CLI 的 `motivation` 命令使用相同接口。管理分页上限 100；自动选择使用独立的条目、完整正文与依据字节预算和真实 continuation。普通 MCP 使用可信启动来源范围读取；不因模型参数获得采用、修订、清除或付费 evaluate 权限。Host 管理 SDK 与插件的真实反馈入口独立于模型 MCP。

普通 Query、自动 Projection 与 trace 使用同一正式引用和 owner 读取。来源许可先于候选预算与正文读取，结构 scope 不授予来源传播。criterion 的读取 selector 与支持某项认识的 basis/dependency 分开：目标状态变化不能因为“希望它结束”而使条件自身失效；读取其状态仍需要实际来源许可。metadata-only 状态路径不授予目标正文。

## 有界条件

表达式只含版本 1 的 eventWitness、ownerState、measurement、relation、semantic，以及 ALL/ANY/NOT。`motivation.criterion_max_depth` 默认 8，`motivation.criterion_max_leaves` 默认 64；修改配置只改变后续采用/修订的工程预算，不删除已经保存的内容。未知版本或任意 SQL、脚本、URL evaluator 被拒绝。

- eventWitness 读取实际 Material Observation 的规范事件事实，匹配事件类型、可选目标/活动和时间域。生成或模拟文本不是已发生外部效果的见证。
- ownerState 读取固定 owner 的正式状态历史。exact revision 固定被引用对象与来源，状态命题读取该 owner 的对象生命周期；它不宣称旧修订仍是当前 head。完整历史只从该修订实际登记与实际首条状态快照均已出现后成立，不能补造迁移前的覆盖。当前支持认知对象的采用/完整性/抑制状态，以及 Runtime WorkContext 的 `open/paused/ended` 状态；WorkContext 不使用 `active`，其他 owner 的状态仍按各自合同验证。
- measurement 保留名称、单位、数值、非负误差和域时间；单位不匹配不做隐含换算，误差区间横跨阈值时未知。
- relation 引用本 Subject 的精确 SocialRelation，核对类型、degree 与该修订的明确有效时间；未指定有效时间不被解释为无限有效。
- semantic 使用固定评价问题和真实材料 key 目录，沿现有 `subject_reflection` 模型绑定运行 `motivation_criterion` operation。输出只含逐项真值、理由和材料 key，没有 coverage 字段。模型不能生成新引用、evaluator 或来源许可。

官方 Client 的 `@nous-wave/client/motivation` 提供 `criterionFactContext` 和 `decodeCriterionFact`，内部使用 canonical Protobuf `CriterionFact`。Host 把它合入真实 Observation.context；它不是另一份事实库。event/measurement 的 `at` 是 DomainStamp，必须同时带事件域时间和现实 recorded 时间，Observation 的实际 observedAt 继续由真实接触产生。

## 时间、覆盖与成果

同一时刻使用 TRUE/FALSE/UNKNOWN 的 Kleene 三值逻辑。条件另外声明 Achieve、Cease、Maintain 或 Avoid、带域时间窗、适用 scope、覆盖要求和可选 hold 时长。窗口采用半开范围。评价返回 applicable（可未知）、SATISFIED/VIOLATED/UNKNOWN、是否已有最终结论、实际已评价 interval、coverage、精确条件/evaluator 身份、来源和生成方式。

Achieve 的窗内正见证可以确定满足；闭窗后没有见证仍须足够覆盖才能判违反。Cease 需要先成立再结束的见证，声明 hold 时还须连续范围覆盖。Maintain 的反例和 Avoid 的禁止事件可以提前确定违反；最终满足必须闭窗且覆盖完整。开放窗口与外部 QQ 观察默认有限；截止时间经过、空消息页或夜间离线不能证明外部世界从未发生某事。

新 owner 在自己的事务内捕获 Subject Clock DomainStamp 并保存 formed/evaluation stamp；Authority recordedAt 始终是现实 UTC。条件自己的 window 带域；裸 validTime 与既有无域 owner 的五轴仍按现实域解释。不同域缺少明确映射时评价未知，不把虚拟课时混同现实 QQ 观察时间。

确定性评价不调用模型。semantic 的实际模型输入、typed 输出、producer 和执行记录先进入现有 model_results，再由 Kernel 读取该成果提交评价。提交中断或重启复用已存成果；目录选择不合法也保留已付费输出，不重新生成。生成器使用当前模型路由与 fallback，没有另一条 Agent 或模型重试循环。来源 purge 沿原传播闭包清理依赖的模型成果及评价内容；普通条件修订/撤回保留旧版本的评价历史。

共享反思的 Motivation 提案与 Memory/Self/Social 使用同一批实际接触、scope/key 目录、模型机会和已保存结果；逐项调用 owner。每个 mutation 必须引用本批真实新来源。一次外部委托不会因为被接受就自动形成终身偏好；愿望存在也不自动创建 Pursuit、承诺或外部动作。

## 验证范围

`cargo test -p nous-kernel --test motivation` 使用隔离 PostgreSQL、认证 Kernel/Core、公开 Client 和本地确定性 provider，检查对象/修订、真实来源、三值与时间边界、有限覆盖、Query/Projection/trace、管理分页之外的选择、普通 MCP 权限以及独立进程恢复已保存的 semantic 成果。确定性 provider 验证协议与恢复，不证明真实模型的判断质量；真实 QQ Pursuit 和有用工作的端到端行为另由 Host 验证。
