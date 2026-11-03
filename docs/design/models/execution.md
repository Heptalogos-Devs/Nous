# 统一模型请求与结果

## 执行责任

ModelExecutor 完成生成请求，统一使用所选 Provider 协议实现、客户端、预算、取消和观测。它不建立认知对象，不决定记忆是否成立，不从模型输出取任意系统操作去执行。

采用 Pydantic AI 的原生模型与消息能力。文本直接请求可以用 direct API；需要结构化结果时使用其已有输出支持及 Pydantic 校验。按固定任务调用，不引入自主工具循环、规划 Agent 或图编排层。

## 核心操作

```text
prepare(task, input, scope, config_snapshot) -> PreparedGeneration
execute(prepared, operation_context) -> GenerationResult
stream(prepared, operation_context) -> async result stream
```

PreparedGeneration 是进程内不可变值，包含 task、选定模型/预设、CompiledPrompt、输出合同、有效参数、能力匹配和预算。它不创建新的持久准备/接纳/托管状态系统。

准备完成后，执行只使用该值。操作过程中修改配置不改变当前请求。fallback 需要另一模型时，针对下一候选重新做兼容性和预算准备，并形成对应 attempt；不把旧 Provider 请求体直接交给另一协议。

## 正式结果

GenerationResult 返回实际输出、模型/Provider 身份、生成时间、任务和模板/schema 版本、实际输入引用、提供方 request_id、用量、结束原因及 attempts。未返回的 token/价格字段标为未知。

提供结构化解析失败时保留原始输出的受控读取位置和错误说明，供当前调试；不自动将失败文本作为正式认知。成功结果由调用领域采用，生成出处随被采用内容保存。临时 trace/call diagnostics 可以淘汰。

## 原生消息与媒体

沿用成熟库对 system、user、assistant、tool、reasoning 和 media parts 的模型。业务 TaskInput 只提供用途与内容，Protocol wire 由 Provider adapter 解释。

三种媒体策略由上游任务安排：description_only、direct_structured、describe_then_structure。第二种和第三种的结构化调用读取实际任务需要的媒体；第三种同时可读已有描述。每次请求有各自生成出处，不把两次调用伪装成一个 producer。

Embedding 有独立 executor 与空间合同，复用网络和预算，不把文本生成输出伪装为向量。

## 流式行为

尚未向调用者交付任何输出时可按 fallback 策略尝试下一候选。已经交付流内容后失败，返回部分结果与明确中断，不能静默接入另一模型继续拼成一条成功消息。

流由调用者持有异步上下文，取消或离开必须关闭响应。工具调用结果与调用 ID 的完整交接由实际宿主负责；本次基础生成不自行执行模型返回的工具动作。

## 预算与失败

所有真实请求共享[预算](budgets.md)，尝试顺序遵循[fallback](fallback.md)。底层 SDK 的自动重试显式关闭或统一纳入同一计数，禁止叠加。

模板错误、能力不足、输入超限在联网前返回。Provider 调用失败保留真实状态。模型任务不能因为失败自动扩大预算、换成更昂贵模型或降低来源要求。

## 完成条件

通过真实网关完成文字生成、一个小结构化结果和流式取消；实际模型身份与用量可以查看；任务不写入未经调用方采用的认知；缺模型时基础实例仍可使用。付费调用需配置与授权，未运行项按实际报告。

依据：[Pydantic AI Direct](https://ai.pydantic.dev/direct/)、[Output](https://ai.pydantic.dev/output/)。
