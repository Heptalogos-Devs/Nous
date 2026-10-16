# Subject 生活世界时钟

[返回合同目录](../../INDEX.md)

## 世界消费者与完成范围

世界/日程/已实现事件由 Heptalogos WorldLife 拥有；Nous 拥有唯一认知时钟、Material 与认知时间。worldId 标识世界，timeDomainRef 标识时间域，两者不互相替代。默认同一 Subject 一个主世界；时区用于日历解释，持久记录时间使用现实 UTC。

本合同随首个正式世界消费者贯通：受信世界可见事件 → Material 接触 → Episode/Journal → Self/Social/Motivation → Query/Projection。时间 owner、SDK 或隔离 PG 单独通过不代表持续世界完成。WorldDefinition、realize、隐藏场景、日程和 NarrativeCommit 仍由其 Host owner 实现，不在 Nous 再建世界运行时或资格平台。

## 所有权与接口

`runtime::SubjectClockService` 复用 `CognitiveClock`，为每个 Subject 保有一个主世界。认证的 `CognitiveClockService` 经 Core 转发给 Kernel；官方 Client 的 `clocks` 命名空间与 CLI `clock read|configure|pause|advance|resume` 使用同一合同。普通 MCP 连接不能调用改钟操作，只有管理连接可以；模型参数不会授予管理连接权限。

默认域为 `time:real`，速率为 1。首次修改可显式配置世界域；直接暂停或推进默认钟时建立 `time:subject:<subject UUID>`。一旦选择显式世界域，后续只能改变该域的速率或向前推进；换世界或回到过去使用隔离 Subject。当前内核为映射的唯一在线 owner，不支持多个 Kernel 同时管理同一 Subject。

`configureClock` 接受 `subjectId/operationId/timeDomainRef/rate/worldTime?`；`pauseClock` 接受身份；`advanceClock` 加非负 `seconds`；`resumeClock` 加正 `rate`。速率必须有限，时间不能倒退或溢出。重复 operation ID 返回原提交的完整结果，不再次推进；同 ID 改变输入会被拒绝。读取返回当前世界时间、现实记录时间、当前段锚点、速率和 revision。

## 映射、停机与持久化

在线时间为 `worldAnchor + rate × monotonicElapsed`，使用 `Instant`；暂停为零。变速先捕获当前世界时刻，再建立新段。返回值精度为 PostgreSQL 微秒精度，防止纳秒截断造成恢复倒退。

`subject_clocks` 保存现实 checkpoint 与已提交世界高水位，`subject_clock_changes` 保存操作的精确重放结果。重启从高水位恢复；明确暂停保持世界时间，其余情况停机期间按现实 1 倍经过时间推进，之后恢复配置的在线速率。现实校时回退产生的负经过时间取零。Clock read 本身写入 checkpoint；单纯调用同步 `CognitiveClock::now` 只读取进程视图，不构成持久提交。

新的识别时间域的 Rust owner 在自己的 Authority 事务中调用一次 `clocks.capture_in(tx, subject)`，同时保存返回的 `DomainStamp` 和本次业务结果。checkpoint 与结果一起提交或回滚。该操作应先于事务内其他 Clock 操作，以保持 Subject clock 锁顺序。Host Life 可读取同一正式 API 的 `now` stamp，并将它保存在自己的活动/事件结果中。

## 五轴与适用域

`DomainStamp` 包含 `timeDomainRef/at/recordedAt`；最后一项始终是现实 UTC。`DomainTemporalExtent` 包含域与数学 `extent`，允许未知及开放区间。数值相同的不同域没有隐式映射。Rust `DomainStamp.compare` 与 Client `compareDomainStamps` 在跨域时返回未知。

Material 的 `occurredTime` 使用带域 extent；每个实际 Occurrence 保留自己的 `observedStamp`、现实 `observedAt` 和现实记录时间。真实 QQ、网页与工具的显式历史 `observedAt` 保持 real 接触，不能把后来 admission 的世界时间倒填为过去接触。缺省同步接触由 owner 捕获当前 stamp。正式 `world_realized` 接触另保存角色在故事中的 seen/heard 点与实际现实交接时间；只有受信 WorldLife 释放路径可提供该叙事内观察点，普通模型或来源字符串不能授予此身份。真实 QQ/tool 接触可另由认证 Host 提供实际接触当次官方 Clock 捕获的 observedStamp；保留独立 observedAt，并校验 mapping 的域与实际捕获时间。没有当时 mapping 的历史接触保持 real，不事后倒推。派生表示和 DerivedRegion 分别保存自己的形成 stamp；分段不会继承父表示的形成时刻。

`TemporalEvidence.sources` 是具体 `SourceTemporalEvidence` 行，每行绑定一个实际 occurrence 的 occurred、observed 与现实 observed/recorded。检索不能用一个接触的发生时间配合另一个接触的观察时间虚构共同见证。即使多个接触共享 Artifact，也在当前 SourceRead 视图中逐实际 occurrence 检查许可。`Hit.temporal` 和 `ContextSegment.temporal` 使用同一 `DomainTemporalEvidence`，包含这些来源行、valid、formed 和现实 recorded。

Memory/Schema/Episode/Journal/Self/Social 的 `formedAt` 为 DomainStamp；形成坐标、域和形成捕获时的现实时间独立持久，owner `recordedAt` 仍是本次 authority 提交的现实时间。已保存模型结果的 cognitiveFormedAt 保留原形成 stamp；恢复不会重新生成形成时刻。默认 real 的内部身份编码不增加域字段，非 real 域参与语义身份。迁移只升级明确的 canonical owner 目录、计划和回执，不修改原模型输出、输入文本、raw/paid/native transcript 或语义 request ID。

Memory/Schema/Association 的 valid 域有正式列；Self/Social/Motivation 的 valid 域属于其正式内容。Episode `experienceTime`、Journal `temporalScope` 也带域。父子包含、同轨重叠、修复邻域和 Journal 聚合只比较同域跨度；未映射的混合 Journal 范围为 Unknown。叙事范围不是新的实际观察事实。

Self/Social/Motivation 自身拥有 formed、valid 与现实 recorded；它们不把来源事件时间冒充自身 occurred/observed。要求这些不适用轴的硬筛选不会命中。来源事件仍由实际 Material Occurrence 表达。未知 valid 可用于当前 standing 选择，但不是显式 valid 时间谓词的正见证。

## 世界见闻与现实数据库历史

角色的 `knowledgeAtWorldTime` 表示已实现故事截至指定世界时刻的可见见闻；`asRecordedAt` 表示数据库在现实提交截点实际持有的记录。二者是不同读取参数，可以同时限定，不能把一个裸 timestamp 或 formed 谓词当成另一问题。

晚实现的虚拟事件可在早晨生成并正式释放昨夜的角色见闻：occurred/世界观察点指向昨夜，realized、现实 observed/交接与 recorded 如实指向早晨。世界见闻切面可使用已合法回填的接触；早于实际入库的现实 as-of 不得读取它。回填必须兼容已确定事件、角色已表达的知情/不知情和相关 Claim；不相容时拒绝或作为当前新发现/新回忆处理。真实 QQ、网页、工具接触不适用虚拟回填规则。

相对词在理解/采纳时解析到明确域与区间，保留原话。过去发生、当时知道和现在如何理解过去分别选择事件范围、角色见闻切面与认知解释版本。身份/角色视角与来源许可先于时间候选、Serving、reranker 和正文预算；隐藏世界状态不会因时间范围匹配而变成 Subject 知识。

离线默认 simulate_elapsed：按已存现实锚点以 1 倍经过时间推进，online 加速率不自动用于 offline。显式选择沿在线速率离线推进时由正式时钟合同保存政策。pause_with_host 正常停止先暂停；异常退出从持久高水位恢复，不能猜测精确关机时刻。未确定区间交给有界 world realization，不伪造停机期间的外部工具或 QQ 效果。Clock 控制不改写已发生历史。

## Query、投影与反思

每次 Query 冻结一个语义 `clockNow: DomainStamp` 和独立现实 authority cutoff。TimePredicate 可显式给出 `timeDomainRef`；未指定的语义轴使用捕获的参考域，recorded 与历史 `asRecordedAt` 始终使用现实 UTC。NousQL 的语义相对日期基于世界坐标，记录/历史相对日期基于现实记录坐标；`$time(..., domain=...)` 可显式限定域。跨域合取不会按裸数字相交。

当前选择、时间检索和历史候选先在原 owner SQL 内做域、具体来源联合谓词与许可过滤，再应用候选预算或读正文。Social/Motivation 显式历史 valid 范围替代当前 standing 点筛选；布尔分支可保守扩大授权元数据候选，最终仍由原 Query 树判断。

Projection 输出同一次捕获的 `semanticClock`。自动 Self/Social/Motivation 选择复用它；实际 renderer、Projection Steward 与 reflection 计划读取正式时间元数据。官方 Client 的严格 prepare 要求该 stamp，缺失时在 provider 前明确失败，独立模式也相同。时间戳经官方 protobuf JSON codec 进入内容指纹与增量比较，时间证据变化不会被漏掉；钟每次读取的变化不强迫所有既有段重置。

Authority history 在原 `authority_object_states` 同时保存 semantic 提交 stamp 与现实 recorded/order。OwnerState 及 relation evaluator 只在同域实际历史中判断；缺 mapping、历史缺口或异域来源返回 UNKNOWN。外部 QQ 流仍然是 open/limited/gap，deadline 过去不构成完整覆盖。

认知 idle 与 settle 使用同域实际接触/提交 stamp。派生的现实 `due_at` 只决定调度何时重新检查；暂停或未映射世界不会因现实轮询过去而变成熟。正式 Clock 控制后唤醒已有认知义务重新判断，不授予模型/执行预算，也不清除 `retry_not_before`。网络 deadline/backoff、worker lease、QQ platformSentAt/receivedAt 继续使用现实/单调基础设施时钟。

外部 producer 必须一次性升级 canonical ObservationInput JSON：旧裸 occurredTime 包成 `{timeDomainRef:"time:real", extent:旧值}`，保留请求身份；不保留旧 decoder。H 的 registration contacts、contact responses 和 pending feedback 属于其自身迁移 owner。真实来源只有接触时取得的正式 mapping 才能关联 world 观察点；不能回推历史 QQ 消息。受信晚实现虚拟见闻沿前述世界合同接纳，不能被这一现实来源限制一刀拒绝。

## 日程投影与命令

官方 Client 导出 `projectWorldDeadline(clock, dueStamp)`，按读取的当前段把世界 due 转成现实唤醒时间。暂停且尚未到期返回 `paused`；域不匹配或缺数据返回 `unmapped`。调用方在变速、暂停和恢复后重新读取并重新投影已有 Activity job；辅助函数不自行创建定时器或模型工作。

pause/resume 的 generated 请求可携带 `expectedRevision`（int64）；Host 生命周期必须用实际 clock read 或暂停 receipt 捕获的 revision。Owner 在同一 Authority 事务和 subject-clock 锁内比较持久 revision，冲突不改钟。相同 operationId 与相同请求先重放原 receipt，再考虑当前 revision，因此重放不会撤销后来 operator 的修改。不填该字段表示显式无条件管理，原请求摘要保持不变。

CLI 读取使用 `nous clock read --subject <reference>`；configure/advance/resume 通过 `--request-file <JSON>` 传入上述 generated 请求字段。Subject 与 operationId 仍由当前 CLI 选择与操作身份机制提供。Client 例如：

```ts
await client.clocks.configureClock({
  subjectId, operationId, timeDomainRef: "time:course", rate: 60,
});
const clock = await client.clocks.readClock({ subjectId });
await client.clocks.pauseClock({ subjectId, operationId: anotherOperationId });
```

## 验证

`cargo test -p nous-kernel --test subject_clock` 启动隔离 PostgreSQL，验证正式认证 gRPC/官方 Client、操作重放、Subject 隔离、进程内加速、真实记录时间和普通 MCP 管理边界，并由独立 OS 进程重开同一数据库验证暂停、高水位及停机 1 倍恢复。测试不连接操作者实例或 QQ，也不调用付费模型。

`cognition_domain_time` 在隔离真实 PG 中验证形成/成果恢复、域隔离、历史 valid 候选预算、纳秒边界、完整 ownerState 窗口及暂停世界 idle 的现实唤醒。`material_query` 验证同一实际接触的联合轴与派生表示/分段各自的形成域。`journal_authority` 验证同域叙事跨度和混合未知范围；历史与公开 longitudinal Core/SDK smoke 验证现实提交 cutoff、维护与成果恢复。

Client 的 `clock.test.ts` 检查 due 投影、暂停和未知域。这些隔离测试不等同于现有操作者 Subject 的加速运行或真实 QQ 外部流完整覆盖证明。

## 当前实现与交付范围

当前实现提供带域五轴、实际 UTC 记录、联合来源时间、同域 owner-state 历史、正式 worldRealized 接触、knowledgeAtWorldTime/asRecordedAt 独立切面与模型上下文。自动 Episode 使用显式域经历时间，跨域关闭分段，同域晚见闻进入局部 repair。持续生活的纵向交付还须配套 WorldLife 消费者与正式角色隔离。隔离 PG/SDK 回归不证明原 Subject 已具备持续世界生活，部署须使用匹配的新组合。
