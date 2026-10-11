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
- Projection/Managed Context reads active WorkContext refs through Kernel and revalidates them before exposure.

Known Memory, Schema, Episode and Journal references remain Subject cognition when supplied in request situation. Current Memory-owner lifecycle validation removes hidden, invalid and purged cognition; a failed materialization removes the segment instead of relabeling an empty reference as external authority. Other canonical references also retain Subject ownership validation. Consumer `memory` policy covers all four cognition families.

Core Projection assigns segment identities from the final selected, bounded content, exact source references, evidence, authority/stability and source revision. Per-read contribution IDs do not change that identity. Managed Context appends only to a synchronized unchanged prefix: a repeated unchanged projection produces an empty APPEND with the same cursor; actual source/content change produces RESET. Tracks are process-local, so restart or an unknown cursor produces RESET.
