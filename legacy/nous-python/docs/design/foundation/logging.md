# 实例日志与宿主出口

## 日志入口与输出所有者

模块在事件发生时使用 `observability.instrumentation.get_logger(__name__)` 取得标准 LoggerAdapter。当前根操作绑定实例路由；关联字段在创建事件时固定。实例自己的 logger 持有文件 handler，向上不传播到宿主 root。模块名作为事件来源保留。

模块不持有业务化的 TraceService，不在 import 时安装日志处理器。基础错误和路径读取无需等待完整观测启动。

## 实例出口

`LoggingSession` 只管理所属 Runtime 的 handler 和等级，不修改 root、全局 structlog 配置或第三方日志等级。未绑定实例的启动信息使用包级 logger，由宿主或 CLI 呈现。

配置可读取、日志路径可用后，所属实例建立 UTF-8 JSON Lines 文件及标准 RotatingFileHandler，轮转参数见[字段目录](configuration-fields.md)。CLI 负责结果、进度与错误的终端呈现；SDK 不接收宿主和其他实例的日志。

新文件 handler 成功建立后替换本实例旧 handler。文件创建失败通过 stderr 说明；未启用文件日志时不显示其他实例的记录作为补齐。

## 正式字段

记录 event、level、logger、实际 UTC 时间、instance_id。操作中另含 operation_id、可用的 trace_id/span_id、operation name、结果与耗时。字段值来自真实上下文。

业务对象引用、来源和模型详情按具体事件必要性增加。默认不记录整份配置、Prompt、模型正文、媒体字节、Cookie 或 Authorization。对凭据引用和受限正文仅给必要状态。

日志不持有正式认知或已采用模型结果的唯一副本。它按轮转和保留策略淘汰。

## 上下文与线程

根操作通过 contextvars 绑定 provider、实例路由与关联字段，退出同一范围时恢复。预先创建的 worker 根据当前调用的 Runtime 进入范围；跨实例嵌套返回后恢复原归属。线程切换沿受管理计算传播上下文。

Formatter 使用事件已携带的关联，不在输出时猜测实例。格式化副本不会改变共享 LogRecord 的消息和异常；宿主显式挂接的其他出口仍能读取原事件。资源持有不保存创建 Task 的 contextvar token，交错关闭实例不依赖关闭顺序。

`stdout` 留给命令结果、JSON 或协议帧。任何后续 MCP stdio 服务也遵守这一输出边界。

## 错误与关闭

错误日志遵循[错误专题](errors.md)。观测输出故障不得递归调用自身导致异常风暴；最小回退写 stderr。关闭最后 flush/close 文件 handler，不删除原日志目录。

完成条件：启动前失败可见；正式日志无重复；中文和异常原因可读；JSON/协议输出不夹带日志；并发操作关联不串线。采用 [stdlib handlers](https://docs.python.org/3.14/library/logging.handlers.html) 与 [structlog stdlib integration](https://structlog.org/en/stable/standard-library.html)。

## 事件与呈现

终端显示有意义的操作、阶段、对象与结果；完整标识在 JSONL 中保留，失败给出可检索关联。实例出口采用 structlog 的标准 formatter，呈现不改变事件的正式字段。

stdlib extra 中的实际业务字段由共同桥接收集，不能只保留 result/duration/error_type 四项。操作开始通常 DEBUG，关键完成、部分完成和失败使用适当级别；网络尝试记录见 [请求尝试](../models/request-attempts.md)。产品状态与日志读取见 [诊断](../interfaces/diagnostics.md)。
