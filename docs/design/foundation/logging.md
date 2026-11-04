# 两阶段日志

## 日志入口与输出所有者

模块直接使用标准 `logging.getLogger(__name__)`。需要结构化绑定时使用 structlog 与 stdlib 集成。处理器、格式、级别和输出位置由观测装配统一建立。

模块不持有业务化的 TraceService，不在 import 时安装日志处理器。基础错误和路径读取无需等待完整观测启动。

## 启动阶段

程序入口首先安装 stderr handler，默认 INFO，纯文本格式包含时间、级别、模块和消息。此时不依赖配置、目录或数据库。取得实例后加入 startup_id，用于与随后正式启动过程关联。

配置可读取、日志路径可用后，将 stdlib 与 structlog 输出接入正式配置。本地文件采用 UTF-8 JSON Lines 和标准 RotatingFileHandler，轮转参数见[字段目录](configuration-fields.md)。终端继续使用适合人阅读的 stderr。

旧 handler 在新处理器可用后移除并关闭，避免重复输出。早期日志已经在 stderr 可见；不维护永久启动日志缓冲重放系统。文件创建失败时保留 stderr 并说明故障。

## 正式字段

记录 event、level、logger、实际 UTC 时间、instance_id。操作中另含 operation_id、可用的 trace_id/span_id、operation name、结果与耗时。字段值来自真实上下文。

业务对象引用、来源和模型详情按具体事件必要性增加。默认不记录整份配置、Prompt、模型正文、媒体字节、Cookie 或 Authorization。对凭据引用和受限正文仅给必要状态。

日志不持有正式认知或已采用模型结果的唯一副本。它按轮转和保留策略淘汰。

## 上下文与线程

入口用 contextvars 绑定关联字段，操作退出时恢复。标准 logging 的桥接 formatter 读取当前关联。同步库的日志也进入同一格式。线程切换采用成熟的 context 传播方式，不以全局可变 dict 保存当前用户或操作。

`stdout` 留给命令结果、JSON 或协议帧。任何后续 MCP stdio 服务也遵守这一输出边界。

## 错误与关闭

错误日志遵循[错误专题](errors.md)。观测输出故障不得递归调用自身导致异常风暴；最小回退写 stderr。关闭最后 flush/close 文件 handler，不删除原日志目录。

完成条件：启动前失败可见；正式日志无重复；中文和异常原因可读；JSON/协议输出不夹带日志；并发操作关联不串线。采用 [stdlib handlers](https://docs.python.org/3.14/library/logging.handlers.html) 与 [structlog stdlib integration](https://structlog.org/en/stable/standard-library.html)。

## 两种呈现使用同一事件

终端列只显示有意义的操作、阶段、对象、结果与耗时；完整标识在 JSONL 中保留，失败给一个可检索关联。使用 structlog 的现成 columns/formatter，处理器不得因为终端隐藏字段而同时删掉文件字段。

stdlib extra 中的实际业务字段由共同桥接收集，不能只保留 result/duration/error_type 四项。操作开始通常 DEBUG，关键完成、部分完成和失败使用适当级别；网络尝试记录见 [请求尝试](../models/request-attempts.md)。产品状态与日志读取见 [诊断](../interfaces/diagnostics.md)。
