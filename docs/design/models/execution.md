# 模型请求与响应执行

## 完整责任

ModelExecutor 负责已确定请求的网络执行、解析、有限尝试、流、用量与错误。GenerationOperations 组织输入 Artifact、模板准备、ModelExecutor 调用及成功结果采用。ModelExecutor 不依赖 CAS、Database、MediaAnalysis 或 CLI。

## 应用操作的输入边界

`GenerationOperations.run(GenerationRequest, context)` 接收 task、按序 inputs（本地文件/InlineText/ArtifactSelection）、question、instructions 及可选自定义 Schema。它完成接纳、确定性的输入准备和请求执行保存。CLI 只调用这项操作，不自己串联 CAS、模板和数据库。

MediaAnalysis 已经准备好一个批次时，调用同一 owner 的内部 `run_prepared(task, ReadingInput, source_map, context)`，复用后半段执行与采用。run 和 run_prepared 收敛到同一核心实现；前者提供有实际作用的接纳/准备，后者避免重复解码。两者都不调用 MediaAnalysis。

直接 model run 只做确定性准备；需要额外转写/描述的输入由 media analyze 显式组织。媒体准备不反向调用 GenerationOperations，因此依赖无环。

## 协议采用

本轮使用 Pydantic AI 的 direct/model API 和它的消息/Schema 适配能力。结构化输出使用该库的 native/tool 输出支持和 Pydantic/标准 JSON Schema 校验。不使用带自主工具执行、planning、反思循环的 Agent Harness；输出工具仅承担结构化结果封装。

## 接口与值

```text
prepare(task, prepared_input, snapshot, context) -> PreparedGeneration
execute(prepared, context) -> GenerationResult
stream(prepared, context) -> async context manager of GenerationStream
transcribe(prepared_audio_request, context) -> TranscriptionResult
```

PreparedGeneration 是进程内值：选定连接/模型、编译消息、输出合同、参数、技术上限和同快照的候选路线。它没有另一个持久 prepared/admitted/reported 生命周期。准备可解析凭据但不发模型请求。

GenerationResult 包括输出、原生响应必要部分、结束原因、实际/请求模型身份、用量、request_id、生成时刻、attempts、模板/Schema 版本和精确输入对应。未知字段保持未知。底层响应拒绝、截断或协议错误须保留实际原因。

TranscriptionResult 是同一模型执行能力的另一种标准端点结果，使用官方客户端 multipart 上传，不强行套成 Chat 消息；来源、期限、用量和网络仍共用现有能力。

## 任务专一与输出

模型得到明确材料与局部键。UUID、事务版本、配置选择和保存动作不由模型推理。Pydantic 任务使用同一类型生成 Schema 与校验返回；自定义 Schema 由 jsonschema 校验。结构化解析失败不采用为成功结果。

若默认一次请求就能完成，不追加确认模型。可配置的结构修复初始为 0；明确启用时每次修复计入该阶段 attempts。普通输出可以是长 Markdown，不强制改成巨大的控制 JSON。

## 操作与阶段

顶层 GenerationOperations 或 MediaAnalysis 取得 Execution 根操作名额与配置快照。内部模型阶段继承同一上下文，不重新取得全局名额。普通参数不能为子操作延长父 deadline。

每个语义阶段有自己的模型尝试计数；描述与结构化是两项计划内阶段，分别可使用既定任务分配。总用量由应用累计，详见 [额度](budgets.md)。禁止 SDK 再做未计数的自动重试。

## 流式结果

CLI 文本流可以逐段交付。尚无任何用户可见内容时，可按 [fallback](fallback.md) 尝试下一候选；交付第一个 delta 后冻结该次输出来源，后续失败返回中断及已有部分，不把另一个模型的内容拼成一次成功结果。

只有收到最终响应且完成程序校验/保存后才发出 completed 事件和成功结果引用。结构化 CLI 默认完整返回；内部即使采用流也在完整解析之后交付正式对象。

响应流使用异步上下文管理。退出、取消和异常都关闭网络资源；CancelledError 保留取消语义。取消前已经提交的独立阶段结果保持可读。

## 配置与输出采用

运行中的配置变更不影响当前请求。fallback 使用根操作冻结的输入和模板，只为新候选重新检查实际协议与参数。未经请求的参数降级不隐藏。

请求前后都不持有写事务。生成完成后，应用发布字节并用 ResultStore 的短事务采用。保存失败不得重新调用模型来补偿数据库；当前已取得结果可用于重新采用或保留本次诊断。

## 使用说明

本轮实际服务是用户授权的本地网关。真实调用不需要再申请费用批准。能力缺失先检查可用网关模型与成熟 SDK，无法满足的具体输入保留可读来源并报告缺项；不声称凭模型目录名已经具备该能力。

依据：[Pydantic AI Direct](https://pydantic.dev/docs/ai/core-concepts/direct/)、[Output](https://pydantic.dev/docs/ai/core-concepts/output/)。
