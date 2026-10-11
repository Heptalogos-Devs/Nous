# Official Client operations

Node consumers connect with `connectNousInstance({ runRoot })` from `@nous-wave/client/node`. The [Client package](../../packages/client/README.md) and canonical [public protocol](../../proto/README.md) define the typed methods. These operations use the same Core and semantic owners as CLI/MCP.

The Client removes transport headers from message DTOs and decodes protobuf Value/Struct at their actual owning types. JSON payloads remain opaque, including keys named `$typeName`, `$unknown` or `subjectId`. First-party consumers use `@nous-wave/client/data` to retain and restore actual protocol schemas without placing them in user JSON.

Persist the operation ID and exact request before mutations. Same ID/same normalized input replays the original result; changed input conflicts. Read the current object epoch/head before a new revision or lifecycle change, then freeze that expected value for retries. A model proposal is not an Authority mutation until its owner commits it.

## Required vocabulary

| Field | Accepted values and meaning |
| --- | --- |
| `subjects.adoptSeed.kind` | `initial` for initial adoption; `import` for an imported version. Omitted/empty means import. |
| `CognitiveSeed.format` | `application/vnd.nous-wave.cognitive-seed+toml;version=1`; text is nonempty TOML with `schema_version = 1`. Seed provenance belongs to the versioned source. |
| `MemoryContent.cognitiveRole` | `experiential`, `declarative`, `procedural_experience`. |
| `MemoryContent.formationMode` | `grounded` or `synthesized`; grounded requires its actual grounding Occurrence, synthesized requires independent supported inputs. |
| `MemoryContent.semanticRole` | A nonempty caller/producer semantic label, such as `working_practice` or `reported_fact`; it is not a closed enum. |
| `MemoryContent.epistemicClass` | Required: `observed`, `reported`, `derived`, `inferred`, `narrative`, `simulated`. This is the claim's epistemic category, not its cognitive role or formation mode. |
| `memory.revise.intent` | `correct`, `rephrase`, `reinterpret`; preserve the same cognition object's referent/scope continuity. |
| `memory.setAccessibility.mode` | `auto`, `normal`, `deep`, `explicit`; accessibility does not authorize suppression/purge bypass. |
| `identity.rebindEntity.bindingState` | `bound`, `unbound`, `disputed`. Bound requires `entityRef`; unbound forbids one. Mention ID belongs to an actual source Occurrence. |
| `model.formFromObservation.aboutnessMode` | `explicit`, `select_from_resolved_mentions`, `none`. Explicit aboutness refs require explicit mode. CLI `--aboutness` chooses explicit unless the caller specifies mode. |
| `Episode.revise.intent` | `resegment`, `reinterpret`; members and sources remain exact references. |
| `CognitiveSchemaContent.formationKind` | `explicit_import`, `synthesized`. Explicit import needs actual evidence; synthesized needs at least two independent known source roots. |
| Schema evidence role | `support`, `counterexample`, `boundary_case`; formation basis role separately uses `direct`, `interpretation`, `contextual`. |

## Source-backed correction and lifecycle

Use `memory.get({ subjectId, id: memoryId })` for current head/epoch and `memory.revision({ subjectId, id: revisionId })` for exact immutable content. `memory.revise` takes `memoryId`, `expectedObjectEpoch`, `operationId`, `intent`, and a complete `MemoryContent` including the actual basis, aboutness, tags and epistemic category. `memory.history` uses `memoryId` and bounded `page`. [NousQL](../agent/NOUSQL.md) provides current/as-of/history reads; current permission and purge fences always apply.

`memory.suppress/restore/withdraw/reaccept/purge` take the same mutation identity fields: `subjectId`, `memoryId`, `expectedObjectEpoch`, `operationId`. Re-read epoch for a new operation; replay uses the old frozen expected epoch. Suppression and withdrawal differ from purge; purge is irreversible. Lifecycle transitions do not create a content revision. Episode and Journal families have their corresponding typed lifecycle methods. WorkContext exact anchors are not silently rewritten when Memory head changes.

`concepts.createSchema/reviseSchema/splitSchema/mergeSchemas/addSchemaEvidence` use the public Schema contracts, exact evidence and object epoch. A schema is a supported transferable pattern with explicit applicability and boundary, not an arbitrary list of remembered facts. `subjects.seed/adoptSeed` read/adopt versioned sources; adoption does not silently replace already committed cognition.

`concepts.suppressSchema/restoreSchema/withdrawSchema/reacceptSchema/purgeSchema` take `subjectId`, `schemaId`, `expectedObjectEpoch`, and `operationId`. They use the Memory owner's lifecycle and dependency fences, without creating a content revision. Purge clears Schema revision content and resident/context/Association references, and revokes incoming Schema evidence links. Provenance and receipt identities may remain; previously recorded UseEvents replay as duplicates through content-free purge receipts. It does not delete shared admitted Material.

`material.occurrences({ subjectId, artifactId, limit })` returns actual observations of an admitted Artifact, newest first, with `truncated`; limit is 1..200. SourceRegions identify content coordinates, not a unique observation. Use this bounded reverse lookup to select the intended Occurrence rather than assuming a nearby retrieval hit is its origin.

Mutation replay preserves the original operation and exact immutable revision. Returned mutable lifecycle state and object epoch describe the object's current state; a delayed suppress replay after restore does not suppress it again. Authorized `memory.get/revision` and Schema management reads expose content with current lifecycle fields for inspection and repair, including withdrawn/suppressed objects. Ordinary NousQL enforces visibility, including exact query targets; management reads do not recover purged content.

## Session, identities and consumers

Session state is isolated; WorkContexts belong to Subject and can continue across Sessions. Pause/end clears affected Session foreground bindings atomically; resume does not foreground a Session. Ended contexts cannot mutate/resume. `cognition.openSession/getSession/listSessions/closeSession` and WorkContext methods use this Runtime owner contract.

`identity.bind/resolve` discover real canonical identities and returned LexicalRefs. Same display names may produce ambiguity; use the chosen returned candidate, never guess a vector identity. `identity.rebindEntity` changes the interpretation of a source mention without rewriting its original surface.

`identity.addresses({ targets: [{ subjectId, canonical: { kind, value } }] })` reads existing directory addresses in one batch, preserving target order. It never allocates an address, adds Subject visibility or changes display names/aliases. Up to 2048 targets are accepted; each result reports `BOUND`, `UNKNOWN_REFERENCE` or `REFERENCE_TOMBSTONED` with a LexicalRef only when visible and live. Merged Tag lookup uses its current canonical address. Explicit `identity.bind` remains the operation for assigning a name or alias.

`cognition.project/managedContext` require a `consumerId` with an operator-configured policy. A valid Subject or Session does not grant access to an unknown consumer. `config` can list/get/describe/set current policy/configuration within its advertised scope; model and resource profile changes require Core restart. Managed-context cursor reuse must preserve consumer identity, policy, source revisions and lifecycle.

## Maintenance and model execution

`cognition.grantMaintenance({ subjectId, maxOperations, maxModelCalls, maxElapsedMs })` gives a bounded opportunity. Subject effective `maintenance.enabled=false` disables it. Response disposition distinguishes disabled policy, no eligible work, processed work and exhausted opportunity. A second opportunity resumes durable needs/proposals; a grant is not a batch with an invented replay receipt. Actual model calls include route fallback.

Model-backed calls have a longer Client default deadline; explicit `timeoutMs` and cancellation take precedence. READY describes executable configured prerequisites, not successful provider calls or prepared Serving. `system.projections({ subjectId })` reports serving state; `model.prepareEmbeddings` builds bounded material using the configured provider batch size. Real query diagnostics and degradation decide whether a requested lane participated.

Explicit maintenance grants accept 1..900000 elapsed milliseconds. Client deadline follows that exact work opportunity plus the active `core_execution.opportunity` cleanup, acknowledgement and response-margin leaf values. CLI/MCP share this Client and the same in-process command core; configure the host tool timeout to cover the returned execution envelope. Per-model execution deadlines and model-call counts still apply to each route, including fallback.

[Reference index](README.md) · [Documentation index](../INDEX.md)
