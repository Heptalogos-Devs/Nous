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

## 时间轴与当前边界

`DomainStamp` 由 `timeDomainRef/at/recordedAt` 组成；`recordedAt` 始终是现实 UTC。`DomainTemporalEvidence` 分别表达 occurred、observed、valid、formed 与现实 recorded：occurred/valid 使用带域的 `TemporalExtent`，允许未知及开放区间；`observedRealAt` 另存实际取得内容的现实时间，不能拿记录时间替代观察时间。不能把同数值、不同域的时间戳直接排序。Rust `DomainStamp.compare` 与 Client `compareDomainStamps` 在域不同或信息缺失时返回未知。

Material 已保存带域 occurredTime、独立 observedStamp，以及派生表示的 formedStamp；observedAt/recordedAt 仍为现实 UTC。默认 real 的观察请求维持原有语义身份编码，非 real 的域参与身份；迁移只更新已知 owner 回执，不改 raw/paid/native transcript。旧数学时间来源接口对非 real 的 occurred 返回 Unknown，不进行跨域数值比较。

Memory/Schema/Episode/Journal/Self/Social 的形成时刻现由同事务 Clock stamp 给出，`formedAt` 对外统一为 DomainStamp。记录中的 formed 时间与域共同构成该轴；recordedAt 保持现实。已保存模型成果的 cognitiveFormedAt 亦为 DomainStamp，恢复沿用原成果，不重新生成时间。0015 只升级已知 canonical reflection 目录和 owner 回执路径，保留语义 digest 和原模型输出文本。

各 owner 的 valid 时间域、完整带域检索与相对日期解析仍待接入。旧裸 formed 查询只接受 real-domain，虚拟域不会伪装 UTC 参与筛选或偏好。现有 owner 的 `CognitiveRuntimeService.now` 保持现实域，不接入虚拟时钟；Material `observed_at` 和 Authority 记录时间不会因世界变速而改变。既有五轴请求没有世界域参数，因此不能用于虚拟域查询，也不能把世界 stamp 填入它们的裸 Timestamp 字段。Host Life 的世界活动应保存正式 domain stamp；跨 owner 的虚拟域查询需要后续完成各 owner 的域存储与过滤合同。

QQ 的 platformSentAt/receivedAt、现实 observedAt、网络 deadline/backoff、worker lease 继续使用现实/单调基础设施时钟。这个服务不改变它们。

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

Client 的 `clock.test.ts` 检查 due 投影、暂停和未知域；现有真实活动、QQ 时序与各认知 owner 的虚拟域查询不由上述测试证明。
