# Nous Wave TypeScript Client

[返回目录](../../INDEX.md)

`@nous-wave/client` is the official typed TypeScript Client for the public Core API. It wraps Connect transport and generated Protobuf types; it is not a separate domain Authority.

- [Package manifest](package.json)
- [Current API reference](../../docs/reference/NOUSQL.md)
- [Administrative operations and required vocabulary](../../docs/reference/CLIENT.md)
- [Protobuf source](../../proto/README.md)

Node consumers use `connectNousInstance({ runRoot })` from `@nous-wave/client/node` for authenticated local discovery. The returned client includes `artifacts.uploadFile(subjectId, path, { mediaType })` and `artifacts.uploadBytes(subjectId, bytes, { mediaType })`. File upload streams with backpressure and an exact multipart length; credentials stay inside the transport. Request options support cancellation and an upload timeout (default 300 seconds).

Model-backed operations derive their response deadline from active `core_execution.opportunity`: work time plus cleanup, acknowledgement and response margin. A maintenance grant uses its requested `maxElapsedMs` as the work budget with the same configured confirmation envelope. Ordinary Node reads retain the 30-second transport default. A shorter caller `timeoutMs` or cancellation bounds the operation; cancellation reaches Core and providers. Callers resume unfinished maintenance needs with another bounded opportunity rather than replaying an imaginary batch receipt.

RPC failures expose `NousError.code` as the generic transport category and `domainCode/recovery/context` from canonical typed error details. Display messages do not carry machine semantics. `details` and identity `candidates` remain available, including unrecognized future details. See the [error and recovery contract](../../docs/specs/active/model-runtime/reference-consumer.md#机器错误与恢复); in particular, `Aborted` alone does not imply stale context.

`client.cognition.prepareQuery({ subjectId, nousql, sessionId?, workContextId?, situation? })` 只准备/inspect，返回 `boundQuery` JSON（resolved query、complete representation、source refs、SHA256、profile、ConfigSnapshot digest），不调用 provider 或构建 Serving。`cognition.query` 共用该 preparation，内部固定 token 后生成一份 query embedding。非空 Unicode 意图接受代词与短 follow-up；显式 Identity/Tag selectors 使用现有 Directory resolver。冻结的 QueryContextSnapshot 包含自由文本、精确 cognition/Entity/Tag anchors、Session/ResidentSet 与 history exclusions。

Text-only 研究使用普通无 context TextCue；生产兼容开关已删除。Memory/Schema/Episode/Journal 的 exact revision API 支持精确续接。

QueryRequest 的 typed `capabilities` 传递 text embedding、multimodal interpretation、residual sensing 与 rerank requirement。`rerank: "forbidden"` 明确关闭 model rerank，适用于 deterministic algorithm/Agent wiring run；`text_embedding: "forbidden"`（Client 为 `textEmbedding`）禁止 embedding provider。Required 需求的失败不静默回退。

Tag owner 操作使用 `concepts.reviseTag/mergeTags/splitTag`。所有 target 带 `tagId/expectedRevisionId`，mutation 带 `operationId/subjectId`；merge/split 带 exact cognition `RevisionBasis`。Tag 响应包含 `currentRevisionId/status/canonicalTagId`。merge 保持 survivor，split 返回 child Tags；历史关联不会被批量改写。

Concept maintenance 的 model suggestion list 由 Core 逐项调用 canonical Tag/Association owner APIs；Client 没有 batch concept/consolidation envelope。Tag lineage basis 接受 owner 验证的 Evidence 或 cognition revision；create/revise/split/association 可携带 structured ProducerSignature。每项 mutation 的稳定 operation ID 保证恢复时精确 replay 已提交结果。
