# 网络客户端、协议连接与生命周期

## 所有权

Network 根据纯配置创建和复用与当前 SDK 配套的 HTTP 客户端（`httpx2.AsyncClient`），管理代理、证书、超时和连接池。ModelConnections 组合成熟协议客户端及对应网络资源。业务模块不直接 new 第二套 SDK 或读隐藏环境。

首轮一个兼容 Provider 连接在运行时内复用，各个模型和生成/Embedding/Transcription API 使用同一连接资源。无需为不同上游厂商创建连接类。实际 provider-specific 协议映射由第三方 SDK 或网关承担。

## 连接创建

endpoint 明确为协议 base URL，例如实际网关的 `/v1` 根，不填 `/chat/completions` 完整路径。使用 `OpenAIProvider(openai_client=AsyncOpenAI(...))` 等成熟注入方式，把正式 HTTPX client 与 `max_retries=0` 交给 SDK。

传输由唯一 owner 关闭。多个模型借用同一协议连接，不逐模型调用 SDK.close 误关闭共享 HTTPX；运行时结束后先停止使用者，再关闭协议/网络资源一次。配置中需要重建客户端的参数按重启采用。

network.trust_env 默认为 false。实例或 Provider 选定的代理、CA、池容量与超时实际传给客户端。供应商环境变量不形成旁路。OTLP exporter 自己的网络继续由观测装配管理。

## 请求与期限

模型 POST 的本地重试只有 ModelExecutor 负责。SDK 和 HTTP transport 的自动重试关闭；网关内部重试由网关承担，Nous 记录自己提交的 attempts，不声称知道网关隐藏的实际上游调用数。

HTTP 分项 timeout 与操作剩余期限共同约束等待；流式读取保留关闭句柄。收到取消时关闭未消费响应，取消不启动 fallback。非模型文件接纳本轮先支持本地文件/字节，远程 URL 下载不因模型能力建设自动扩张。

## 依赖和错误

客户端完成网络 I/O，应用在 I/O 前后各使用自己的短事务。网络错误保持状态码、可用 request_id、连接与限流原因。错误描述中保留可用服务信息，普通输出不打印授权头。

基础模块使用 logging 与轻量 OTel API。SDK 提供的埋点可以接入当前 provider，但不调用全局 auto-instrument-all 或另建遥测控制中心。

依据：[HTTPX Async](https://www.python-httpx.org/async/)、[Pydantic AI OpenAI-compatible](https://pydantic.dev/docs/ai/models/openai/)。
