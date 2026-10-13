# Reference Consumer

[返回当前产品合同](../../INDEX.md)

## Owner 与边界

`apps/nous-cli` 是第一方 consumer，通过 public `@nous-wave/client` 调用 Core。[CLI README](../../../../apps/nous-cli/README.md) 定义可执行命令。Terminal 与官方 stdio MCP 调用同一个 in-process command core；MCP 提供 help、argv 与 NousQL query，stdin/stdout 仅用于协议，不接受文件 `-`。MCP 显式提供私有 state root 与稳定 consumer identity。consumer 是不超过 512 UTF-8 字节的非空、不含空白的 opaque ID；配置、projection、CLI selection 与 UseEvent 使用同一原值，默认 ID 为 default。

Core discovery 由 RunRoot 提供。consumer InstanceRoot 保存 Subject/Session/WorkContext selection、exact result:N 索引与冻结原请求，不拥有 Nous Wave Authority。state 按稳定 Core instanceId 与 consumer 隔离。只读配置直接使用连接；显式 Subject 按需解析。需要当前上下文的操作才读取 selection，需要结果索引或冻结请求才读取 active consumer_state policy；原子写与跨进程锁仅覆盖短事务，RPC/provider 等待不占锁。独立 selection 字段可合并，冲突 Subject/query scope 或同字段更新返回 STATE_UNSAVED，保留实际业务结果和可继续使用的 exact references。

## Canonical DTO 与呈现

Client 方法直接来自 generated service，不维护别名表：runtime 与 cognition、resourceRegistry 与 resources 分别保留各自 owner。配置 Value 由调用者在 JSON 输入边界显式编码。

Client 直接返回 generated Protobuf message，不建立 Data DTO、WeakMap schema 或恢复层。generated `$typeName` 与 descriptor 定义消息字段；配置 Value 在显式配置值边界解码，文本/JSON 在单一呈现边界使用 protocol messages helper。Proto `reference_kind` 标注决定哪些字段可呈现为地址，renderer 只读去重批量 Identity 地址，不分配、改名或增加 visibility。query 已返回的 lexicalRef 直接沿用。freeform Struct/Value 正文在所属字段保持不透明，合法 `$typeName`、`$unknown`、引用同名字段与完整 UUID 均保留。

native JSON 由 Buf toJson 负责：oneof 为实际字段，Timestamp 为 ISO，bytes 为 base64，int64 为十进制字符串。renderer 不按用户对象的 kind/value、case/value、seconds/nanos 形状猜测协议类型。

默认语义文本；`--json` 返回版本化 envelope；`--raw --developer` 查看 canonical Client 响应。完整 owner 字段不会由重复手写 summary 列表隐藏。业务成功但地址呈现失败，输出 canonical result 与 PRESENTATION_UNAVAILABLE；selection 保存失败输出 STATE_UNSAVED。

## 请求恢复

幂等 mutation 在 RPC 前保存完整冻结原请求，包括实际 operation/event identity、expected epoch/revision、时间戳和已解析 canonical references。原请求 identity 不允许被不同输入替换；codec 区分 BigInt 与用户 JSON 字符串。

semantic owner 是唯一结果与拒绝 Authority。consumer 不建立 complete/rejected 状态表。成功响应，以及 owner 明确的提交前拒绝（STALE_REVISION、OPERATION_ID_CONFLICT、InvalidArgument、PermissionDenied、Unauthenticated）删除本地原请求；删除失败只附 REQUEST_UNSAVED notice，不改变业务结果。连接丢失、DeadlineExceeded、Unavailable 或可能发生于提交后的 Internal/Unknown 保留原请求并返回 retry ID，`retry <id>` 将相同原输入再次发送给 owner，不重新读当前 revision、生成 identity 或重新解析名称。Subject creation 重试不要求先选择新 Subject；其他 Subject-bound 重试校验 scope。重试只恢复业务请求，不补做独立的 local selection 更新。

`retry forget <id>` 只删除本地原请求，不撤销 Authority 操作。保存请求容量耗尽时先恢复或明确丢弃原请求；没有第二套终态回收规则。Session lifecycle、Identity binding、derivation、upload 的实际幂等合同由各 owner 决定，不声称全部调用都可用同一方式恢复。

## 可用能力

consumer 公开 Subject/seed、Session、Observation/Material、模型形成/derivation/embedding、NousQL、UseEvent、WorkContext、Resource Registry/materialization、Memory/Schema/Episode/Journal 的实际管理与 lifecycle、projection/managed context。复杂输入采用 owner 内容的语义 TOML，generated schema 校验 declared fields，引用统一绑定一次后冻结；不是动态 raw RPC 执行入口。

Memory/Schema/Episode/Journal 的 get/history/lifecycle 使用稳定对象，revision/trace/use 使用 immutable exact revision。修正与生命周期 command 读取当前 owner epoch，再保存冻结 mutation。WorkContext 保持正式 full replacement 合同，set/pin/unpin 在读取的 expected revision 上更新；structured fields 与分页可显式使用。projection 与 managedContext 使用 operator-configured consumer policy，不与 WorkContext 文本更新混淆。

Material `show` 读取 metadata，`read` 通过 Material Authority 返回 bounded source/derived preview，包括真实范围、总量、partial 与 evidence。Artifact `read --output` 通过认证 Core download 流导出完整字节，使用新文件，不受 preview 上限限制；失败删除本次部分输出。上传使用 authenticated streaming endpoint。

## 机器错误与恢复

领域 owner 在发生处提供稳定业务码与必要上下文，Kernel/Core 使用 [ErrorDetail](../../../../proto/nous/wave/v1alpha1/errors.proto) 传输。Client NousError.code 保留 transport 类别，domainCode/recovery/context 暴露 typed detail，details/candidates 保留附加信息。message 只供显示，不解析其中的 JSON 或业务标识。

STALE_CONTEXT/STALE_REVISION 要求刷新状态；OPERATION_ID_CONFLICT 要求为不同输入使用新 identity。recovery 是调用者建议，不授权额外 mutation。未知 detail/enum 保留原信息，不猜测业务含义。
