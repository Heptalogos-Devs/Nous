# Nous Wave TypeScript Client

[返回目录](../../INDEX.md)

`@nous-wave/client` is the official typed TypeScript Client for the public Core API. 它直接公开各 canonical service 的 generated Connect client 方法，不保留别名或独立 domain Authority。Runtime Session/Observation/UseEvent/WorkContext 属于 `runtime`，Cognition Query/maintenance/projection 属于 `cognition`；Resource Registry CRUD 属于 `resourceRegistry`，外部内容执行属于 `resources`。

- [Package manifest](package.json)
- [Current API reference](../../docs/reference/NOUSQL.md)
- [Administrative operations and required vocabulary](../../docs/reference/CLIENT.md)
- [Protobuf source](../../proto/README.md)

## 独立安装

Heptalogos Host 和其他仓库外 Node 消费者使用正式 tarball，不引用本仓库的 TypeScript 源码：

```sh
# 在 Nous Wave 仓库内构建
corepack pnpm install --frozen-lockfile
corepack pnpm client:pack

# 在 Host 项目内安装生成的包
npm install /absolute/path/nous-wave-client-0.1.0.tgz
```

产物为 `data/releases/client/nous-wave-client-0.1.0.tgz`，要求 Node >=24.21.0。包提供 `@nous-wave/client`、`/node`、`/managed`、`/errors`、`/consumer-policy`、`/execution-policy` 的 ESM JavaScript 和 `.d.ts`；无需 tsx 或源码 checkout。官方 `@nous-wave/protocol` 的同源编译代码和 generated 声明作为 bundled dependency 随 Client 一起安装，其余精确 npm dependencies 正常安装。tarball 仅含编译代码、类型、包元数据和许可证，不包含实例配置、凭据或数据。

```js
import { connectNousInstance } from "@nous-wave/client/node";

const client = await connectNousInstance({ runRoot: "/absolute/path/instance/run" });
const status = await client.system.getStatus({});
const subjects = await client.subjects.listSubjects({});
```

TypeScript Host 使用 `module/moduleResolution: "NodeNext"` 并安装 `@types/node`；所有服务请求/响应保留 canonical generated 类型。`NodeNousClient` 和 `NousInstanceClient` 是 `/node` 的正式返回类型，`ArtifactUploadOptions`、`UploadedArtifact` 也从该入口公开。

维护者执行 `corepack pnpm client:verify`，会重新打包、在系统临时目录的独立 npm 项目安装，逐一加载全部入口，并在 `skipLibCheck: false` 下验证 generated 类型与下游声明生成。该检查已接入 `check:fast`。`corepack pnpm smoke:client` 复用[公共 smoke](../../scripts/smoke/README.md) 的隔离实例，用安装后的正式 Client 执行 status、Subject create/get/list，不调用付费模型；已接入 `just smoke`。独立消费者成功后清理，失败时打印并保留路径。

Client returns generated canonical Protobuf messages directly. Decode configuration `Value` fields explicitly with `toJson(ValueSchema, value)`; the workspace CLI uses `messageData` from `@nous-wave/protocol/messages` at text/JSON output boundaries. The bundled Protocol is an internal Client dependency, rather than a separate Host import entry. Freeform JSON leaves remain opaque.

Node consumers use `connectNousInstance({ runRoot })` from `@nous-wave/client/node` for authenticated local discovery. The returned client includes `artifacts.uploadFile(subjectId, path, { mediaType })` and `artifacts.uploadBytes(subjectId, bytes, { mediaType })`. `artifacts.download(subjectId, artifactId, options?)` returns the admitted media type and a backpressured content stream for complete downloads. File upload streams with backpressure and an exact multipart length; credentials stay inside the transport. Request options support cancellation and an upload timeout (default 300 seconds).

Model-backed operations derive their response deadline from active `core_execution.opportunity`: work time plus cleanup, acknowledgement and response margin. A maintenance grant uses its requested `maxElapsedMs` as the work budget with the same configured confirmation envelope. Ordinary Node reads retain the 30-second transport default. A shorter caller `timeoutMs` or cancellation bounds the operation; cancellation reaches Core and providers. Callers resume unfinished maintenance needs with another bounded opportunity rather than replaying an imaginary batch receipt.

RPC failures expose `NousError.code` as the generic transport category and `domainCode/recovery/context` from canonical typed error details. Display messages do not carry machine semantics. `details` and identity `candidates` remain available, including unrecognized future details. See the [error and recovery contract](../../docs/specs/active/model-runtime/reference-consumer.md#机器错误与恢复); in particular, `Aborted` alone does not imply stale context.

`client.cognition.prepareQuery({ subjectId, nousql, sessionId?, workContextId?, situation? })` 只准备/inspect，返回 `boundQuery` JSON（resolved query、complete representation、source refs、SHA256、profile、ConfigSnapshot digest），不调用 provider 或构建 Serving。`cognition.query` 共用该 preparation，内部固定 token 后生成一份 query embedding。非空 Unicode 意图接受代词与短 follow-up；显式 Identity/Tag selectors 使用现有 Directory resolver。冻结的 QueryContextSnapshot 包含自由文本、精确 cognition/Entity/Tag anchors、Session/ResidentSet 与 history exclusions。

Text-only 研究使用普通无 context TextCue；生产兼容开关已删除。Memory/Schema/Episode/Journal 的 exact revision API 支持精确续接。

QueryRequest 的 typed `capabilities` 传递 text embedding、multimodal interpretation、residual sensing 与 rerank requirement。`rerank: "forbidden"` 明确关闭 model rerank，适用于 deterministic algorithm/Agent wiring run；`text_embedding: "forbidden"`（Client 为 `textEmbedding`）禁止 embedding provider。Required 需求的失败不静默回退。

Tag owner 操作使用 `concepts.reviseTag/mergeTags/splitTag`。所有 target 带 `tagId/expectedRevisionId`，mutation 带 `operationId/subjectId`；merge/split 带 exact cognition `RevisionBasis`。Tag 响应包含 `currentRevisionId/status/canonicalTagId`。merge 保持 survivor，split 返回 child Tags；历史关联不会被批量改写。

Concept maintenance 的 model suggestion list 由 Core 逐项调用 canonical Tag/Association owner APIs；Client 没有 batch concept/consolidation envelope。Tag lineage basis 接受 owner 验证的 Evidence 或 cognition revision；create/revise/split/association 可携带 structured ProducerSignature。每项 mutation 的稳定 operation ID 保证恢复时精确 replay 已提交结果。

配置读取使用 `configuration.getConfiguration({ paths, ... })`，descriptor 使用 `getConfigDescriptor({ path })`；override 使用 canonical `setSystemOverride/setSubjectOverride`，value 在调用者的 JSON 输入边界通过 `fromJson(ValueSchema, value)` 编码。字段保持 generated request 形状。普通 RPC 与模型 deadline 使用一个方法 decorator 保留 typed NousError、显式 CallOptions 和取消；传入的 Transport 不要求重建。
