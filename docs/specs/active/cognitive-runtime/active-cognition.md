# Active Cognition

[返回文档目录](../../INDEX.md)

## Owner

Runtime (crates/runtime) owns this Subject/Session view; Core Projection and Managed Context consume it.

The executable Active Cognition view is:

`ResidentSet(session) ∪ WorkContextRefs(session.active_work_context) ∪ request.situation.current_refs`

References are deduplicated by canonical exact `CognitiveRef` while source family remains available for diagnostics. Current owner validation removes foreign, invalid, and purged refs. Domain owners retain durable Authority; Runtime composes their exact references into the Active Cognition view.

- retrieval hits alone do not add residency;
- meaningful UseEvent may update ResidentSet under the existing idempotent contract;
- WorkContext refs are runtime relevance input, not meaningful use;
- runtime query returns resident and active WorkContext candidates with source diagnostics;
- Projection/Managed Context 使用已捕获 foreground WorkContext 的任务文本与 anchors；各领域 owner 解析并核验实际来源，Runtime 的 Query Representation 复用同一任务文本。非空 constraints、恢复条件和预算摘要只表达任务内容，不驱动执行策略或自动恢复。

Memory、Schema、Episode 与 Journal 即使由 request situation 提供，仍属于 Subject cognition。Memory owner 的生命周期核验排除隐藏、失效和已清除来源；读取失败会移除段，不把空引用重新标为外部 Authority。Memory 同时提供 active canonical Tag 语义。其他 canonical refs 仍核验 Subject 归属；Entity 的名称和别名来自已有目录，未绑定时仅保留引用。Consumer 的 `memory` policy 覆盖四类 cognition 和 Tag 语义。

Core Projection assigns segment identities from the final selected, bounded content, exact source references, evidence, authority/stability and source revision. Per-read contribution IDs do not change that identity. Managed Context appends only to a synchronized unchanged prefix: a repeated unchanged projection produces an empty APPEND with the same cursor; actual source/content change produces RESET. Tracks are process-local, so restart or an unknown cursor produces RESET.

各 owner 返回同一个 `ContextSource`，分别表达已解析来源引用与可选的真实内容修订标记。Cognition 和 Tag 使用选中 revision UUID，WorkContext 使用捕获的整数 revision。Immutable Material、Resource descriptor 和 Entity 目录名称不虚构修订标记。
