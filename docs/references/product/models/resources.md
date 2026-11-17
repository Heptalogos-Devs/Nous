# 模型资源、任务分配与协议能力

## 目标

通过用户配置的统一网关完成文字、结构化、媒体理解、音频转写和文本 Embedding。产品不固定上游模型或厂商，也不在运行中探测余额。缺少某项模型不影响内容读取、实例管理和其他已配置能力。

## 共同对象

Provider 保存 ID、明确协议、API base URL、credential_ref 和网络覆盖。Model 保存该连接上的实际 model_name、声明部署版本、上下文/输出能力及输入格式。Preset 保存有效参数。TaskAssignment 选择资源、预设、Prompt、输出模式、fallback 和技术限制。

本轮生产协议为 `openai-chat-completions`。该协议连接还可通过成熟客户端使用标准 `/embeddings`、`/audio/transcriptions` 和 `/models`。这些端点是否可用由实际连接与模型能力确定，不能把一个 endpoint 字符串自动当成所有功能均可用。

连接采用 `pydantic-ai-slim[openai]` 和其支持的官方 SDK。明确构造 `OpenAIChatModel` 与自定义 base_url，避免库的简写默认随版本切到另一个协议。Pydantic AI 的 Profile/Schema 转换直接采用；确需覆盖时使用明确协议兼容选项，不根据模型名写厂商 switch。

## 配置与兼容性

沿用现有 providers/models/presets/tasks 值模型，补齐 [字段专题](../foundation/configuration-model-media.md)。配置 ID 是小写 slug；网关模型名保持原样。

capabilities 表示已经确认的操作能力；input_formats 说明可接受的实际 MIME；output_modes 指定 text/native/tool。`structured_output` 这类概括声明仍需落到具体输出模式。无法匹配时准备阶段返回能力缺项，不能通过移除参数悄悄改变所请求的语义。

`nous tools model list` 读取本地配置。`nous tools model discover` 显式调用所选 Provider 的模型目录，得到可选模型名；目录存在并不证明视觉、音频、原生 Schema 或维度，真实调用按相应能力确认。探测使用同一执行资源，没有独立脚本 SDK 路径。

## 输入中立

任务提供文本、媒体及输出目的。Provider/SDK 负责协议线格式；MediaPreparer 负责通用转换。声明 `video` capability 和实际视频 MIME（如 `video/mp4`）的路线通过 `video_url` 内容扩展提交原生视频。现有 Pydantic AI OpenAIChatModel 缺少该扩展的编码，`models.protocol.CompatibleChatModel` 仅补充 BinaryContent 视频到 data URI 的映射，其余编码、SDK请求、流及资源仍沿用同一实现。该扩展要求所配网关/模型实际支持，不根据模型名选择。未声明视频 MIME 的视觉路线继续由媒体 owner 提供抽帧表示。

本轮无需支持全部远程供应商 SDK。以后新协议真实接入时增加一个依赖 adapter，保持既有任务输入、结果、网络、配置和来源合同。

## 操作

`ModelRegistry.resolve(task, snapshot)` 产生确定的资源选择及 fallback 候选，不联网。

本地模型列表读取 ConfigSnapshot.values.models；`ModelExecutor.discover(provider, snapshot)` 使用统一连接读取远端目录。`GenerationOperations.probe(model, input, snapshot)` 为指定资源准备明确任务；普通调用使用 run_input(task, input, snapshot) 或 run(task, ReadingInput, snapshot)，共享准备、请求及采用通路。资源列表与 status 不隐式生成付费请求。

持久语言任务经 load_language_task 核对当前模型定义，以及 provider 的 endpoint、protocol 和 credential_ref；不一致返回 model.resource_binding。凭据引用指向的密钥内容可以轮换，任务参数和模板继续取自冻结配方。

实际调用保存 requested_model_name、returned_model_name（可空）、protocol、provider 连接 ID 和操作者声明的 deployment_revision。不能根据相同字符串声称取得了同一模型权重。

依据：[Pydantic AI OpenAI-compatible 与 Profile](https://pydantic.dev/docs/ai/models/openai/)、[New API 模型接口](https://docs.newapi.ai/en/docs/guide/feature-guide/user/api)。
