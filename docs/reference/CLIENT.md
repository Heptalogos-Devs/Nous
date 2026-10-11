# Official Client operations

Node consumers connect with `connectNousInstance({ runRoot })` from `@nous-wave/client/node`. The [Client package](../../packages/client/README.md) and canonical [public protocol](../../proto/README.md) define the typed methods. These operations use the same Core and semantic owners as CLI/MCP.

Successful Client calls return the generated protobuf-js response message directly. Callers keep the generated field names, `bigint` 64-bit values, bytes, oneofs and unknown binary wire fields; there is no recursive `Data<T>` conversion or implicit schema-restoration step. Use `toJson(schema, message)` / `fromJson(schema, json)` only at an explicit ProtoJSON boundary. Configuration setters accept generated `SetSystemOverrideRequest` / `SetSubjectOverrideRequest` messages with `google.protobuf.Value` fields; optional `configurationValue(text)` parses JSON text at that explicit helper boundary and returns the generated `Value`. The Client does not implicitly convert plain JSON objects into request DTOs; WKT values remain protobuf messages. `protocolReferences(message, schema)` walks only real Buf messages and protocol-annotated reference fields; plain objects and lookalike `$typeName` keys are not messages.

`cognition.prepareQuery(request)` returns generated `PreparedQueryResponse`, whose `boundQuery` is a typed `QueryInspection`, not a JSON DTO. It is read-only inspection and does not return a `QueryRun` continuation handle. A formal `cognition.query` is executed by Core/Kernel in its internal short-lived `QueryRun`; that run retains its bound query and frozen read context through host work and final validation, then ends.

Use `create(schema, init)` for hand-built protobuf requests and responses. CLI summaries that intentionally omit fields are ordinary DTOs, not generated messages. A ProtoJSON receipt stores the registered request/response type name and complete typed JSON; it does not preserve unknown binary wire fields that ProtoJSON cannot represent.

提交 mutation 前保存 operation ID 和精确 owner request。相同 owner request 可重放原子提交的结果；同一操作身份携带不同 request 会冲突。创建修订或变更 lifecycle 前读取当前 object epoch/head。模型调用直接把 typed output 交给 canonical owner，不另存 proposal 供续接。CLI 的手动 retry 可能再次调用模型；输出变化时同一 owner operation ID 可能冲突。

`system.operationStatus({ identity })` 只读取已有 owner receipt，支持的 typed identity 为 `mutationReceipt`、`subjectCreate`、`configurationMutation` 或 `observationRequest`；它不查询或续接模型调用。

## Required vocabulary

| Field                                     | Accepted values and meaning                                                                                                                                              |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `subjects.adoptSeed.kind`                 | `initial` for initial adoption; `import` for an imported version. Omitted/empty means import.                                                                            |
| `CognitiveSeed.format`                    | `application/vnd.nous-wave.cognitive-seed+toml;version=1`; text is nonempty TOML with `schema_version = 1`. Seed provenance belongs to the versioned source.             |
| `MemoryContent.cognitiveRole`             | `experiential`, `declarative`, `procedural_experience`.                                                                                                                  |
| `MemoryContent.formationMode`             | `grounded` or `synthesized`; grounded requires its actual grounding Occurrence, synthesized requires independent supported inputs.                                       |
| `MemoryContent.semanticRole`              | A nonempty caller/producer semantic label, such as `working_practice` or `reported_fact`; it is not a closed enum.                                                       |
| `MemoryContent.epistemicClass`            | Required: `observed`, `reported`, `derived`, `inferred`, `narrative`, `simulated`. This is the claim's epistemic category, not its cognitive role or formation mode.     |
| `memory.revise.intent`                    | `correct`, `rephrase`, `reinterpret`; preserve the same cognition object's referent/scope continuity.                                                                    |
| `memory.setAccessibility.mode`            | `auto`, `normal`, `deep`, `explicit`; accessibility does not authorize suppression/purge bypass.                                                                         |
| `identity.rebindEntity.bindingState`      | `bound`, `unbound`, `disputed`. Bound requires `entityRef`; unbound forbids one. Mention ID belongs to an actual source Occurrence.                                      |
| `model.formFromObservation.aboutnessMode` | `explicit`, `select_from_resolved_mentions`, `none`. Explicit aboutness refs require explicit mode. CLI `--aboutness` chooses explicit unless the caller specifies mode. |
| `Episode.revise.intent`                   | `resegment`, `reinterpret`; members and sources remain exact references.                                                                                                 |
| `CognitiveSchemaContent.formationKind`    | `explicit_import`, `synthesized`. Explicit import needs actual evidence; synthesized needs at least two independent known source roots.                                  |
| Schema evidence role                      | `support`, `counterexample`, `boundary_case`; formation basis role separately uses `direct`, `interpretation`, `contextual`.                                             |

## Source-backed correction and lifecycle

Use `cognition.read({ subjectId, reference: { kind, value } })` for an object or an exact immutable revision. Four cognition families share `CognitionHeader` (identity, head, epoch, lifecycle) and `CognitionRevisionHeader` (revision identity, parent, time, producer); domain payloads remain typed. `cognition.list` selects a kind and bounded page; `cognition.history` takes `objectRef` and page. [NousQL](../agent/NOUSQL.md) provides current/as-of/history query semantics.

`cognition.mutate` takes `subjectId`, `objectRef`, `expectedObjectEpoch`, `operationId` and one typed action: suppress, restore, withdraw, reaccept, purge, or Memory accessibility. All four families use this lifecycle path. A new operation reads the current epoch; retry preserves the frozen request. Purge is irreversible; lifecycle changes do not create content revisions. WorkContext exact anchors retain their selected revision.

`memory.revise` retains its domain-specific `MemoryContent` contract, including basis, aboutness, tags and epistemic category.

`concepts.createSchema/reviseSchema/splitSchema/mergeSchemas/addSchemaEvidence` use the public Schema contracts, exact evidence and object epoch. A schema is a supported transferable pattern with explicit applicability and boundary, not an arbitrary list of remembered facts. `subjects.seed/adoptSeed` read/adopt versioned sources; adoption does not silently replace already committed cognition.

Schema purge clears revision content and dependent resident/context/Association references, and revokes incoming Schema evidence links. Content-free provenance/receipt identities and duplicate UseEvent replay remain; shared admitted Material is retained.

`material.occurrences({ subjectId, artifactId, limit })` returns actual observations of an admitted Artifact, newest first, with `truncated`; limit is 1..200. SourceRegions identify content coordinates, not a unique observation. Use this bounded reverse lookup to select the intended Occurrence rather than assuming a nearby retrieval hit is its origin.

Mutation replay preserves the original operation and exact immutable revision. Returned mutable lifecycle state and object epoch describe the object's current state; a delayed suppress replay after restore does not suppress it again. Authorized `cognition.read` calls expose content with current lifecycle fields for inspection and repair, including withdrawn/suppressed objects. Ordinary NousQL enforces visibility, including exact query targets; management reads do not recover purged content.

## Session, identities and consumers

Session state is isolated; WorkContexts belong to Subject and can continue across Sessions. Pause/end clears affected Session foreground bindings atomically; resume does not foreground a Session. Ended contexts cannot mutate/resume. `cognition.openSession/getSession/listSessions/closeSession` and WorkContext methods use this Runtime owner contract.

`identity.bind/resolve` discover real canonical identities and returned LexicalRefs. Same display names may produce ambiguity; use the chosen returned candidate, never guess a vector identity. `identity.rebindEntity` changes the interpretation of a source mention without rewriting its original surface.

`identity.addresses({ targets: [{ subjectId, canonical: { kind, value } }] })` reads existing directory addresses in one batch, preserving target order. It never allocates an address, adds Subject visibility or changes display names/aliases. Up to 2048 targets are accepted; each result reports `BOUND`, `UNKNOWN_REFERENCE` or `REFERENCE_TOMBSTONED` with a LexicalRef only when visible and live. Merged Tag lookup uses its current canonical address. Explicit `identity.bind` remains the operation for assigning a name or alias.

`cognition.project/managedContext` require a `consumerId` with an operator-configured policy. A valid Subject or Session does not grant access to an unknown consumer. `config` can list/get/describe/set current policy/configuration within its advertised scope; model and resource profile changes require Core restart. Managed-context cursor reuse must preserve consumer identity, policy, source revisions and lifecycle.

## Maintenance and model execution

`cognition.grantMaintenance({ subjectId, maxOperations, maxModelCalls, maxElapsedMs })` 提供有界执行机会。Subject effective `maintenance.enabled=false` 时不运行。返回状态区分策略禁用、无可执行 need、已处理与机会耗尽。MaintenanceNeed claim/lease 只协调后台工作；后续 grant 按当前 owner state 重新计划，需要时重新调用模型，不续接旧 proposal。实际模型调用数包含 route fallback。

Model-backed calls have a longer Client default deadline; explicit `timeoutMs` and cancellation take precedence. READY describes executable configured prerequisites, not successful provider calls or prepared Serving. `system.projections({ subjectId })` reports serving state; `model.prepareEmbeddings` builds bounded material using the configured provider batch size. Real query diagnostics and degradation decide whether a requested lane participated.

Explicit maintenance grants accept 1..900000 elapsed milliseconds. Client deadline follows that exact work opportunity plus the active `core_execution.opportunity` cleanup, acknowledgement and response-margin leaf values. CLI/MCP share this Client and the same in-process command core; configure the host tool timeout to cover the returned execution envelope. Per-model execution deadlines and model-call counts still apply to each route, including fallback.

[Reference index](README.md) · [Documentation index](../INDEX.md)
