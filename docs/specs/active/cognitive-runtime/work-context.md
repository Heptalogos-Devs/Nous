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
- Update is complete replacement with expected revision; refs are validated in the same transaction.
- Pause/end lock the WorkContext and all affected Sessions in deterministic order, clear bindings, and increment each affected Session runtime revision atomically.
- Resume only changes paused→open; it never foregrounds a Session.
- Foreground requires an open Session, same Subject, open WorkContext, and expected runtime revision.
- Ended contexts remain exact-readable but cannot resume, mutate, or become foreground.

## Persistence and recovery

The Runtime stores typed WorkContext data in the fresh canonical schema and recovers it from the same database root after process restart. Prompt, model hidden state, raw cache, and full chat history are not persisted.
