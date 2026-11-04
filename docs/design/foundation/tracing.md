# Trace 与操作关联

## 依赖方向

模块使用 OpenTelemetry API 创建有意义的 Span；应用组合根配置 SDK 和导出器。没有 SDK 或关闭采样时，调用保持可运行。日志直接使用 logging，不通过 Span 才能写出。

首轮提供配置后的 SDK、操作关联、控制台调试导出和可选 OTLP 导出。默认不要求外部遥测平台。默认本地日志保存操作开始/结果/耗时及 trace_id；export=none 时不承诺保存完整 Span 树。

SDK provider 由 Runtime 持有，并通过当前执行上下文提供给埋点。关闭 Runtime 会刷新并关闭其 provider；同一进程之后可以重新打开实例并构造新的 provider，不修改 OpenTelemetry 的私有全局状态。Span 之间继续使用标准 OpenTelemetry context 关联。

## Span 边界

一次完整公开操作为一个 root 或传入上下文的 child。数据库事务、模板编译、模型尝试、内容读取和持久工作执行在有排查价值时增加子 Span。不给每个字段转换和 helper 自动生成 Span。

启动与关闭分别有有界 Span。早于 SDK 初始化的事件由 startup_id 关联，不伪造已采集的 Span。长期运行进程不持有一个持续几天的 root span。

后台任务记录发起上下文。队列等待通过父关联或 link 表达，实际执行和各次尝试形成新 Span；业务任务 ID 与 Trace ID 分别使用。

## 字段与输出

使用标准属性命名和少量 Nous 操作属性。错误 code、组件、结果、模型资源与真实用量按实际内容添加。默认关闭模型正文采集，详细请求调试需显式启用并遵守保留与凭据排除规则。

导出使用 SDK BatchSpanProcessor 与官方 OTLP exporter。stderr 调试模式明确指定输出 stderr，不能使用 ConsoleSpanExporter 默认 stdout 污染 CLI/MCP。OTLP endpoint、超时及采样来自配置，首轮按重启生效。

遥测 SDK 的传输是它自己的运行资源，由组合根配置和关闭；它不调用业务网络服务来报告该服务自身的失败。导出失败只影响观测，保留本地日志。

## 对外边界

以后 HTTP 接入采用标准 W3C trace context。Trace 标识是诊断关联，不是调用身份或来源授权。正式结果保存自己的引用；Trace 不成为业务查找的必要数据库。

## 完成条件

启动、配置更新和一次真实数据库操作可串成清楚的调用关系；禁用 Trace 后同样完成操作；导出不可达时本地能力继续；结束时在有界时间内 flush。来源：[OpenTelemetry API 与 SDK 分工](https://opentelemetry.io/docs/languages/python/instrumentation/)。

## 真实尝试与排错

同一根操作保持一个稳定 operation_id，子阶段 Span 不再生成新的根关联。生成、转写、Embedding 从共同 policy 创建每次真实请求 Span，记录实际模型资源、尝试、状态、业务错误码、退避/切换原因、耗时与已报告用量。

有用事件采用有界 SpanLimits，不再将 max_events 固定为 0。主要错误在应用边界记录一次；失败重试即使最后成功也保留 attempt 事件。默认完整 Span 没有导出时，状态明确显示未持久保存，JSONL 仍提供过程摘要。

Native 通过粗粒度阶段诊断接入，数值热循环不回调 Python。说明字段与呈现见 [诊断](../interfaces/diagnostics.md)。
