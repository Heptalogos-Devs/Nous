# WorkContext

[返回文档目录](../../INDEX.md)

## Owner

Runtime (crates/runtime) owns WorkContext identity, revisions, persistence and recovery.

WorkContext 是 Runtime owner 的 Subject 级 durable checkpoint。它可以跨 Session 延续，但不是 Memory Authority，也不复制 Memory 内容。

## Shape

- stable `WorkContextId`、`subject_id`、`state`、`purpose`、bounded questions/constraints/resume conditions/budget、`revision`、timestamps；
- state 只有 `open`、`paused`、`ended`；`ended` terminal；
- exact continuation refs 只能是 `MemoryRevision`、`CognitiveSchemaRevision`、`Occurrence`、`EpisodeRevision` 或 `JournalRevision`；
- mutable object id 不能作为 continuation ref；
- Session 只保存 optional foreground binding 和 runtime revision。

## Mutations

Typed public RPCs are `CreateWorkContext`、`GetWorkContext`、`ListWorkContexts`、`UpdateWorkContext`、`PauseWorkContext`、`ResumeWorkContext`、`EndWorkContext`、`SetActiveWorkContext`。

- Create starts at revision 1 and uses the existing durable operation receipt.
- Update 只接受一个 typed intent：设置提供的字段、pin anchors、unpin anchors，或清空文本/anchors/两者。未提供的字段保持不变；空文本、空列表和空对象明确覆盖对应字段。Runtime 在锁内核验 expected revision、应用意图并验证 refs。
- List 采用 WorkContextId 升序的有界 keyset 分页；cursor 绑定 Subject，返回实际 next_page_token。
- Pause/end lock the WorkContext and all affected Sessions in deterministic order, clear bindings, and increment each affected Session runtime revision atomically.
- Resume only changes paused→open; it never foregrounds a Session.
- Foreground requires an open Session, same Subject, open WorkContext, and expected runtime revision.
- Ended contexts remain exact-readable but cannot resume, mutate, or become foreground.

## Persistence and recovery

The Runtime stores typed WorkContext data in the fresh canonical schema and recovers it from the same database root after process restart. Prompt, model hidden state, raw cache, and full chat history are not persisted.

## Reactor关键继续信息

WorkContext保存认知目的、问题、约束、anchors和有界继续信息；Life保存产品活动/焦点/时间及成果，两侧引用同一真实结果，不复制完整JSON。新目标/委派让出/切换/取消/完成前同步保存，不依赖异步摘要或反思完成。结果交接失败沿既有稳定待回流身份补交，不重跑已成功工具或付费工作。原生摘要、继续信息与长期认知各自拥有；主观前台由Host组织，Nous不建立平行Reactor。
