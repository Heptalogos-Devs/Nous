# Official Client operations

所有 Client 方法采用 public Proto 的 canonical RPC localName，没有 convenience aliases。Namespace 直接对应 semantic service：subjects、runtime、cognition、memory、material、identity、concepts、configuration、model、resourceRegistry、resources、system。

Node consumers connect with `connectNousInstance({ runRoot })` from `@nous-wave/client/node`. The [Client package](../../packages/client/README.md) and canonical [public protocol](../../proto/README.md) define the typed methods. These operations use the same Core and semantic owners as CLI/MCP.

Client 直接返回 canonical generated Protobuf message，包括 `$typeName`、BigInt 和 typed `Value`；没有平行 DTO、WeakMap schema 或恢复层。调用者在明确的配置值边界使用 `toJson(ValueSchema, entry.value)`；文本/JSON 输出边界使用 `@nous-wave/protocol/messages` 的 `messageData`。Struct/Value 正文保持不透明，包括 `$typeName`、`$unknown` 和 `subjectId` 等合法用户键。

configuration override 的 `value` 使用 generated `Value`，调用者用 `fromJson(ValueSchema, JSON value)` 在明确输入边界编码；`getConfigDescriptor` 接收 `{ path }` request。

Persist the operation ID and exact request before mutations. Same ID/same normalized input replays the original result; changed input conflicts. Read the current object epoch/head before a new revision or lifecycle change, then freeze that expected value for retries. A model proposal is not an Authority mutation until its owner commits it.

## Required vocabulary

| Field                                     | Accepted values and meaning                                                                                                                                              |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `subjects.adoptCognitiveSeed.kind`        | `initial` for initial adoption; `import` for an imported version. Omitted/empty means import.                                                                            |
| `CognitiveSeed.format`                    | `application/vnd.nous-wave.cognitive-seed+toml;version=1`; text is nonempty TOML with `schema_version = 1`. Seed provenance belongs to the versioned source.             |
| `FormMemoryRequest.cognitiveRole`         | `experiential`, `declarative`, `procedural_experience`; creation-only object role.                                                                                       |
| `MemoryContent.formationMode`             | `grounded` or `synthesized`; grounded requires its actual grounding Occurrence, synthesized requires independent supported inputs.                                       |
| `MemoryContent.semanticRole`              | A nonempty caller/producer semantic label, such as `working_practice` or `reported_fact`; it is not a closed enum.                                                       |
| `MemoryContent.epistemicClass`            | Required: `observed`, `reported`, `derived`, `inferred`, `narrative`, `simulated`. This is the claim's epistemic category, not its cognitive role or formation mode.     |
| `memory.reviseMemory.intent`              | `correct`, `rephrase`, `reinterpret`; preserve the same cognition object's referent/scope continuity.                                                                    |
| `memory.setAccessibility.mode`            | `auto`, `normal`, `deep`, `explicit`; accessibility does not authorize suppression/purge bypass.                                                                         |
| `identity.rebindEntity.bindingState`      | `bound`, `unbound`, `disputed`. Bound requires `entityRef`; unbound forbids one. Mention ID belongs to an actual source Occurrence.                                      |
| `model.formFromObservation.aboutnessMode` | `explicit`, `select_from_resolved_mentions`, `none`. Explicit aboutness refs require explicit mode. CLI `--aboutness` chooses explicit unless the caller specifies mode. |
| `Episode.revise.intent`                   | `resegment`, `reinterpret`; members and sources remain exact references.                                                                                                 |
| `CognitiveSchemaContent.formationKind`    | `explicit_import`, `synthesized`. Explicit import needs actual evidence; synthesized needs at least two independent known source roots.                                  |
| Schema evidence role                      | `support`, `counterexample`, `boundary_case`; formation basis role separately uses `direct`, `interpretation`, `contextual`.                                             |

## Source-backed correction and lifecycle

Use `memory.getMemory({ subjectId, id: memoryId })` for current head/epoch and `memory.getMemoryRevision({ subjectId, id: revisionId })` for exact immutable content. Creation and revision use the same writable `MemoryContent`, including the actual basis, aboutness, tags and epistemic category. `memory.formMemory` separately supplies the creation-only `cognitiveRole`; `memory.reviseMemory` adds `memoryId`, `expectedObjectEpoch`, `operationId` and `intent`, without changing that object role. `memory.listMemoryRevisions` returns the same complete Memory view as exact revision reads, with current object lifecycle/epoch and historical revision content. It uses `memoryId` and bounded `page`, ordered by revision number. [NousQL](../agent/NOUSQL.md) provides current/as-of/history reads; current permission and purge fences always apply.

`memory.suppressMemory/restoreMemory/withdrawMemory/reacceptMemory/purgeMemory` take the same mutation identity fields: `subjectId`, `memoryId`, `expectedObjectEpoch`, `operationId`. Re-read epoch for a new operation; replay uses the old frozen expected epoch. Suppression and withdrawal differ from purge; purge is irreversible. Lifecycle transitions do not create a content revision. Episode and Journal families have their corresponding typed lifecycle methods. WorkContext exact anchors are not silently rewritten when Memory head changes.

`concepts.createCognitiveSchema/reviseCognitiveSchema/splitCognitiveSchema/mergeCognitiveSchemas/addSchemaEvidence` use the public Schema contracts, exact evidence and object epoch. A schema is a supported transferable pattern with explicit applicability and boundary, not an arbitrary list of remembered facts. `subjects.getCognitiveSeed/adoptCognitiveSeed` read/adopt versioned sources; adoption does not silently replace already committed cognition.

`concepts.suppressCognitiveSchema/restoreCognitiveSchema/withdrawCognitiveSchema/reacceptCognitiveSchema/purgeCognitiveSchema` take `subjectId`, `schemaId`, `expectedObjectEpoch`, and `operationId`. They use the Memory owner's lifecycle and dependency fences, without creating a content revision. Purge clears Schema revision content and resident/context/Association references, and revokes incoming Schema evidence links. Provenance and receipt identities may remain; previously recorded UseEvents replay as duplicates through content-free purge receipts. It does not delete shared admitted Material.

`material.listOccurrences({ subjectId, artifactId, limit })` returns actual observations of an admitted Artifact, newest first, with `truncated`; limit is 1..200. SourceRegions identify content coordinates, not a unique observation. Use this bounded reverse lookup to select the intended Occurrence rather than assuming a nearby retrieval hit is its origin.

Mutation replay preserves the original operation and exact immutable revision. Returned mutable lifecycle state and object epoch describe the object's current state; a delayed suppress replay after restore does not suppress it again. Authorized `memory.getMemory/getMemoryRevision` and Schema management reads expose content with current lifecycle fields for inspection and repair, including withdrawn/suppressed objects. Ordinary NousQL enforces visibility, including exact query targets; management reads do not recover purged content.

## Session, identities and consumers

Session state is isolated; WorkContexts belong to Subject and can continue across Sessions. Session views expose opened/activity/meaningful-use times, `closedAt` and complete resident entries with state, reason and hold times; `closedAt` is the single closing-state field. Pause/end clears affected Session foreground bindings atomically; resume does not foreground a Session. Ended contexts cannot mutate/resume. `runtime.openSession/getSession/listSessions/closeSession` and WorkContext methods use this Runtime owner contract. WorkContext updates contain one typed `fields`, `pin`, `unpin` or `clear` intent; the owner applies it under the expected revision. Optional fields preserve omitted values; supplied empty values clear their field. `fields.unresolvedQuestions` and `fields.resumeConditions` use `{ values: [...] }` for list presence. WorkContext lists use bounded pages; public Page size is 1..200 (omitted/zero defaults to 50).

`identity.bindIdentity/resolve` discover real canonical identities and returned LexicalRefs. Same display names may produce ambiguity; use the chosen returned candidate, never guess a vector identity. `identity.rebindEntity` changes the interpretation of a source mention without rewriting its original surface.

`identity.getIdentityAddresses({ targets: [{ subjectId, canonical: { kind, value } }] })` reads existing directory addresses in one batch, preserving target order. It never allocates an address, adds Subject visibility or changes display names/aliases. Up to 2048 targets are accepted; each result reports `BOUND`, `UNKNOWN_REFERENCE` or `REFERENCE_TOMBSTONED` with a LexicalRef only when visible and live. Merged Tag lookup uses its current canonical address. Explicit `identity.bindIdentity` remains the operation for assigning a name or alias.

`cognition.buildProjection/buildManagedContext` require a `consumerId` with an operator-configured policy. A valid Subject or Session does not grant access to an unknown consumer. `config` can list/get/describe/set current policy/configuration within its advertised scope; model and resource profile changes require Core restart. Managed-context cursor reuse must preserve consumer identity, policy, source revisions and lifecycle.

## Maintenance and model execution

`cognition.grantMaintenance({ subjectId, maxOperations, maxModelCalls, maxElapsedMs })` gives a bounded opportunity. Subject effective `maintenance.enabled=false` disables it. Response disposition distinguishes disabled policy, no eligible work, processed work and exhausted opportunity. A second opportunity resumes durable needs/proposals; a grant is not a batch with an invented replay receipt. Actual model calls include route fallback.

Model-backed calls have a longer Client default deadline; explicit `timeoutMs` and cancellation take precedence. READY describes executable configured prerequisites, not successful provider calls or prepared Serving. `system.getProjectionStatus({ subjectId })` reports serving state; `model.prepareEmbeddings` builds bounded material using the configured provider batch size. Real query diagnostics and degradation decide whether a requested lane participated.

Explicit maintenance grants accept 1..900000 elapsed milliseconds. Client deadline follows that exact work opportunity plus the active `core_execution.opportunity` cleanup, acknowledgement and response-margin leaf values. CLI/MCP share this Client and the same in-process command core; configure the host tool timeout to cover the returned execution envelope. Per-model execution deadlines and model-call counts still apply to each route, including fallback.

## CLI 与 MCP 的管理入口

官方命令与 MCP 共用同一个 command core。`memory` 支持形成、当前/精确修订读取、历史、源支持修正、accessibility、revision link 与 lifecycle；`schema` 支持形成、evidence、修订、split/merge 与 lifecycle；`episode` 和 `journal` 支持当前/精确读取、分页历史与各自 lifecycle。`resource put/get/list/remove/materialize` 使用真实 Registry 与 Material owner。`projection build/managed` 使用已配置 consumer 的真实投影/上下文合同，不是 WorkContext 文本更新。

复杂内容通过 `--request-file` 语义 TOML 输入，采用 owner 声明的 snake_case 或 camelCase 字段；引用字段接受 canonical、LexicalRef 或唯一名称，`CognitiveRef` 可直接用字符串。`[[basis]] ref="obs:..." role="direct"` 表达证据。Memory/Schema content 文件不包含 operation/Subject/epoch；命令固定本次 Subject、生成或使用 `--operation-id`，并读取 owner 当前 epoch 后冻结。Episode 创建/修订文件使用其内容字段，命令覆盖 Subject/operation/epoch。`--kind` 指定修订 intent 或 accessibility mode；pagination 使用 `--page-size` 和 `--page-token`。

`subject seed/adopt-seed` 读取或采用种子，采用文件包含 `text`、`format` 和所需 provenance；`identity rebind --request-file` 明确修改 source mention interpretation。`context create/set --request-file` 可以输入 unresolved_questions、constraints、resume_conditions、budget_summary；set 列表字段采用 `{ values = [...] }` 表达未提供与清空的区别。Session 与 Context list 支持真正的有界分页。

本地仅保存 selection、exact result 索引和 RPC 前的冻结原请求。`retry <id>` 再次请求 semantic owner，由 owner 返回幂等结果；收到成功响应后删除本地原请求。不缓存成功/拒绝终态，不根据 transport 数字码建立第二个业务状态机。`retry forget <id>` 只删除本地重试输入；不会撤销 Authority 操作。源 Artifact 的 `read <art:...> --output <new-file>` 通过 authenticated download 流导出完整字节，不受 preview 的 1 MiB 上限限制；失败不留下部分输出。

[Reference index](README.md) · [Documentation index](../INDEX.md)
