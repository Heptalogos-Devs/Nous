# Reference Consumer

[返回当前产品合同](../../INDEX.md)

## Owner

`apps/nous-cli` is the first-party reference consumer. It uses the public `@nous-wave/client` package and its Node entry point; consumer behavior is documented in the [CLI README](../../../../apps/nous-cli/README.md).

## Boundary

The consumer connects through Core discovery in RunRoot. It stores the selected Subject, Session and WorkContext under InstanceRoot; these selections are local consumer state, not Nous Wave Authority.

Parallel Agents share Core RunRoot and use independent consumer InstanceRoots. The stdio MCP entry belongs to this same consumer and requires an explicit private state root and stable consumer identity. It exposes help, argv command execution and NousQL query through the official MCP SDK. Terminal and MCP call the same in-process command core and public Client; Terminal alone writes human stdout/stderr and supplies stdin content. MCP stdin remains protocol-only; file `-` input is rejected there.

本地 Consumer state 按稳定 Core `instanceId` 和 consumer identity 区分。每条命令冻结一次 selection、context 和输入。原子写与跨进程锁只保护短本地事务，RPC 与 provider 执行仍可并发。Selection patch 合并独立字段，并拒绝互相冲突或 Subject/query context 已变化的更新。业务操作成功但 selection 保存失败时，仍返回结果并附 `STATE_UNSAVED`；返回的 exact refs 可继续使用。`consumer_state` Catalog policy 定义回执容量、文件预算与锁时间，并在命令开始时从 active configuration 读取。

Selection 是经 Zod schema 校验的普通 JSON。回执保存 request/response schema name 和 Buf ProtoJSON payload；schema 来自生成的 service descriptor。不支持的 selection record 会返回重新选择错误，不改动文件。有限本地 request/result cache 在发送前保存冻结的 typed request，成功后保存返回结果。`retry` 必须显式调用并重发缓存 request，不是自动崩溃恢复；模型请求可能再次调用模型，输出变化时会与同一 owner operation identity 冲突。

Observation/query/trace 返回的 Material ref 按 exact canonical reference 接收。`show` 读取 metadata；`read` 经 Material Authority 物化有界 source/derived text，并保留实际范围、总字节数、partial 标记和 evidence。Binary source 引导调用方先执行 derivation，不直接将原始 bytes 当作文本显示。

`session show [reference-or-name]` 可读取 open/closed Session metadata，不会选择或重开 Runtime activity。省略参数时读取 consumer 当前选择的 Session。Close 会清除本地 Session selection，但保留 closed identity 供该命令读取。Open/close 不接受 reference 参数。

Consumer 使用公开 Client API 操作 Subject/Session lifecycle、Observation、Material derivation、Memory formation、embedding preparation、query、trace、UseEvent 和 WorkContext。文件上传通过认证过的 Core endpoint 流式传送 Artifact content。

Consumer 使用 citty 0.2.2 命令族、Zod 校验的 semantic TOML 输入、默认 semantic text 与可选 versioned JSON。Raw Client DTO 诊断需要 `--raw --developer`。本地状态保存 exact `result:N` refs；有界 request/result cache 仅用于显式手动 retry。Context set/pin/unpin/clear/pause/resume/select/foreground 使用公开 WorkContext API。Consumer 基于公开 Client 构建。Provenance trace 通过公开 Material 以及 exact Memory/Schema/Episode/Journal revision read，从 revision 沿 producer 和 evidence refs 追到来源 Artifact。

正文、title、摘要和操作消息逐字保留，包括完整 UUID 或 `memory:<UUID>`。引用呈现按 typed reference 字段转换。地址呈现不可用时，输出 canonical result 并附 `PRESENTATION_UNAVAILABLE` notice；本地完成回执写入失败则附 `RECEIPT_UNSAVED` notice。

引用字段由 canonical Proto 的 `reference_kind` 标注，Official Client 保留对应 schema，renderer 只读取 Identity 的去重批量地址结果。各创建 owner 在 Authority 事务内分配对象/revision 地址；普通显示不分配、不改名、不增加 visibility。Protobuf JSON Value/Struct 的内容保持不透明，合法 `$typeName`、`$unknown` 和引用同名字段均保留。回执使用 Buf ProtoJSON 保存与恢复正式协议消息；引用类型来自 schema 标注，用户 JSON 始终保持不透明。

## 机器错误

领域 owner 在错误发生处指定稳定业务码与必要的身份、revision 上下文；Kernel/Core 将其作为 [ErrorDetail](../../../../proto/nous/wave/v1alpha1/errors.proto) typed detail 传输。Connect/gRPC code 表示通用类别，message 只供显示。Official Client 的 `NousError.code` 保留 transport code，`domainCode/recovery/context` 暴露已解码的业务语义，`details/candidates` 保留附加信息。CLI JSON 输出使用该语义，不解析 message 中的 JSON 或错误标识；没有业务 detail 的 Aborted 仍显示 `ABORTED`。

`STALE_CONTEXT` 与 `STALE_REVISION` 表示请求所依据的当前状态需要刷新；`OPERATION_ID_CONFLICT` 表示同一 owner operation identity 携带了不同请求。对于不同语义的新请求，使用新的操作 ID。`MaintenanceNeed` 的 claim/lease 只协调后台维护 worker；CLI 不保存或续接模型 workflow checkpoint。未知 detail/enum 保留原信息，调用者不得猜测其业务含义。
