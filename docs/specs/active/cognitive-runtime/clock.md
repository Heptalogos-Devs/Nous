# Subject 生活世界时钟

[返回合同目录](../../INDEX.md)

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

Material 的 `occurredTime` 使用带域 extent；每个实际 Occurrence 保留自己的 `observedStamp`、现实 `observedAt` 和现实记录时间。显式历史 `observedAt` 标为 real 接触 stamp，不能把后来 admission 的世界时间倒填为过去接触。缺省同步接触由 owner 捕获当前 stamp。派生表示和 DerivedRegion 分别保存自己的形成 stamp；分段不会继承父表示的形成时刻。

`TemporalEvidence.sources` 是具体 `SourceTemporalEvidence` 行，每行绑定一个实际 occurrence 的 occurred、observed 与现实 observed/recorded。检索不能用一个接触的发生时间配合另一个接触的观察时间虚构共同见证。即使多个接触共享 Artifact，也在当前 SourceRead 视图中逐实际 occurrence 检查许可。`Hit.temporal` 和 `ContextSegment.temporal` 使用同一 `DomainTemporalEvidence`，包含这些来源行、valid、formed 和现实 recorded。

Memory/Schema/Episode/Journal/Self/Social 的 `formedAt` 为 DomainStamp；形成坐标、域和形成捕获时的现实时间独立持久，owner `recordedAt` 仍是本次 authority 提交的现实时间。已保存模型结果的 cognitiveFormedAt 保留原形成 stamp；恢复不会重新生成形成时刻。默认 real 的内部身份编码不增加域字段，非 real 域参与语义身份。迁移只升级明确的 canonical owner 目录、计划和回执，不修改原模型输出、输入文本、raw/paid/native transcript 或语义 request ID。

Memory/Schema/Association 的 valid 域有正式列；Self/Social/Motivation 的 valid 域属于其正式内容。Episode `experienceTime`、Journal `temporalScope` 也带域。父子包含、同轨重叠、修复邻域和 Journal 聚合只比较同域跨度；未映射的混合 Journal 范围为 Unknown。叙事范围不是新的实际观察事实。

Self/Social/Motivation 自身拥有 formed、valid 与现实 recorded；它们不把来源事件时间冒充自身 occurred/observed。要求这些不适用轴的硬筛选不会命中。来源事件仍由实际 Material Occurrence 表达。未知 valid 可用于当前 standing 选择，但不是显式 valid 时间谓词的正见证。

## Query、投影与反思

每次 Query 冻结一个语义 `clockNow: DomainStamp` 和独立现实 authority cutoff。TimePredicate 可显式给出 `timeDomainRef`；未指定的语义轴使用捕获的参考域，recorded 与历史 `asOf` 始终使用现实 UTC。NousQL 的语义相对日期基于世界坐标，记录/历史相对日期基于现实记录坐标；`$time(..., domain=...)` 可显式限定域。跨域合取不会按裸数字相交。

当前选择、时间检索和历史候选先在原 owner SQL 内做域、具体来源联合谓词与许可过滤，再应用候选预算或读正文。Social/Motivation 显式历史 valid 范围替代当前 standing 点筛选；布尔分支可保守扩大授权元数据候选，最终仍由原 Query 树判断。

Projection 输出同一次捕获的 `semanticClock`。自动 Self/Social/Motivation 选择复用它；实际 renderer、Projection Steward 与 reflection 计划读取正式时间元数据。官方 Client 的严格 prepare 要求该 stamp，缺失时在 provider 前明确失败，独立模式也相同。时间戳经官方 protobuf JSON codec 进入内容指纹与增量比较，时间证据变化不会被漏掉；钟每次读取的变化不强迫所有既有段重置。

Authority history 在原 `authority_object_states` 同时保存 semantic 提交 stamp 与现实 recorded/order。OwnerState 及 relation evaluator 只在同域实际历史中判断；缺 mapping、历史缺口或异域来源返回 UNKNOWN。外部 QQ 流仍然是 open/limited/gap，deadline 过去不构成完整覆盖。

认知 idle 与 settle 使用同域实际接触/提交 stamp。派生的现实 `due_at` 只决定调度何时重新检查；暂停或未映射世界不会因现实轮询过去而变成熟。正式 Clock 控制后唤醒已有认知义务重新判断，不授予模型/执行预算，也不清除 `retry_not_before`。网络 deadline/backoff、worker lease、QQ platformSentAt/receivedAt 继续使用现实/单调基础设施时钟。

外部 producer 必须一次性升级 canonical ObservationInput JSON：旧裸 occurredTime 包成 `{timeDomainRef:"time:real", extent:旧值}`，保留请求身份；不保留旧 decoder。H 的 registration contacts、contact responses 和 pending feedback 属于其自身迁移 owner。只有真实接触时取得的正式 mapping 才能形成 world 接触证明；不能回推历史 QQ 消息的虚拟接触时刻。

## 日程投影与命令

官方 Client 导出 `projectWorldDeadline(clock, dueStamp)`，按读取的当前段把世界 due 转成现实唤醒时间。暂停且尚未到期返回 `paused`；域不匹配或缺数据返回 `unmapped`。调用方在变速、暂停和恢复后重新读取并重新投影已有 Activity job；辅助函数不自行创建定时器或模型工作。

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
