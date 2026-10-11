# Runtime & Use

[返回文档目录](../../INDEX.md)

## Owner

Runtime (crates/runtime) owns Session, ResidentSet, UseEvent, accessibility evaluation and restart-visible Runtime state. Memory owns durable Memory Authority.

## Session isolation

- 一个 Subject 可同时服务多个 Session 与 consumer；durable cognition 归 Subject 共享。
- Session 的 foreground WorkContext binding、ResidentSet、runtime revision 和 activity 按 Session 隔离；WorkContext identity 属于 Subject，可以跨 Session 延续。
- Runtime candidates 只来自指定 Session 的 exact resident revisions 与当前 Situation refs；Subject membership 不能替代 residency。
- Session 属于同一 Subject 且未 closed 才能参与带 Session 的 runtime operation；closed/foreign session 返回 `FAILED_PRECONDITION`。
- 同 Subject 的 `GetSession` 是 metadata read，可读取 closed Session，不选中、不重新打开或推进 activity/runtime revision。

## UseEvent

UseEvent identity 为 `(subject_id, consumer_ref, event_id)`，`event_id` 由 caller 提供。Canonical digest 覆盖 Session、exact cognition revision、use kind、occurred time 和 canonical metadata。

ReportUse batch 先按 key 分组：同 key/same digest 的同批输入 coalesce；same key/different digest 使整批 conflict。数据库已有 event 或 purge receipt 且 digest 相同时是 duplicate，不重新验证已 purge target，也不产生 Runtime side effect。

只有 newly accepted events 改变 Session/ResidentSet。`presented` 可写 durable event 并更新 activity，但不进入 ResidentSet；`referenced`、`acted_on`、`result_supported`、`result_refuted`、`corrected`、`pinned` 等 meaningful use 才刷新 exact resident ref 与 meaningful-use time。duplicate-only request 不改变 activity、resident 或 runtime revision；同一 batch 的多个新 event 只推进一次 runtime revision。

UseEvent target 只接受 exact Memory/Schema/Episode/Journal revision，不接受 mutable object id。Context 仅保存受限 metadata，不保存 prompt、正文或 raw artifact。

Episode/Journal meaningful use 可进入 ResidentSet，检索命中本身不增加 residency；其访问使用 current lifecycle，年龄衰减仅用于 Memory/Schema。

## Accessibility

Accessibility 是 query-time policy，不是 Memory truth、lifecycle 或 purge。当前 reference 参数为 `epsilon=0.02`、`tau_days=30`、`decay=0.5`、`formation_weight=1.0`；typed use weight 和 Normal/Deep/Explicit threshold 由 typed policy 固定。

`auto`、`normal`、`deep`、`explicit` override 只决定当前普通 query 的可访问性。Exact explicit object/revision 与合法 provenance/management read 可绕过 auto level，但不得绕过 Subject ownership、suppression 或 purge。

## Recovery

Session、UseEvent receipt、ResidentSet 和 Authority sequence 必须在同一 data root 重启后可恢复；BoundQuery 不跨进程持久化。Runtime state 丢失时可从 Authority 重建，不能把缓存当作 durable cognition。WorkContext 只保存 bounded purpose/questions/constraints/resume conditions/budget 与 exact refs；prompt、raw cache、model hidden state 和 copied Memory 不持久化。


## Query feedback

ReportUse 可携带 Query 返回的 query_id。Runtime 校验同 Subject、实际返回的 exact revision membership 与七天 reference retention；digest 包含 query_id。Feedback record 是 bounded operational signal，清理不删除 accepted UseEvents；same-digest duplicate 在过期后仍不重新执行 side effect。Presented 不进入 Concept review；result_refuted 作为负反馈单独计数，不加强正向关系。Planner 最多消费八条近期 bounded activation/hypothesis records；它们不是事实支持，也不授予 model activity。

## Historical context

TemporalFrame 的相对时间与 accessibility 采用 captured Subject CognitiveClock；historical accessibility 排除截点后的 UseEvents。WorkContext future cognition refs 在 activation 前剔除，当前 purpose 保留；Subject/current permission/purge fence 不随历史 view 回退。Operational timeout、lease 与 cleanup 使用基础设施时间。
