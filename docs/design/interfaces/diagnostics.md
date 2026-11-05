# 可读结果、操作诊断与能力状态

## 日常呈现

终端显示用户做的事情、对象名称、实际阶段、结果、耗时和必要短引用。完整 instance/startup/operation/trace/span ID 留在结构化日志；失败时给一个可检索的 operation 引用及可用 trace 关联。不是在每条 INFO 后打印全部标识。

正常业务结果优先正文、排名、简要来源与短地址，复杂参数和输入关系通过 --details/--json 展开。列表/嵌套结果使用对应产品 renderer，不把整个 dict 用通用 format_status 倾倒到终端。

## 入口

继续使用现有 `runtime check`，增加详情而不另开管理体系。新增 `runtime logs --operation <id-or-unambiguous-prefix>` 读取现有轮转 JSONL，展示该操作的摘要、阶段、各请求尝试和错误。前缀不唯一则显示候选；不创建新的诊断数据库。

`runtime check` 不自动调用模型。状态来自实际 owner：

| 项目 | 输出 |
|---|---|
| 配置 | 有效/保存版本，待重启，绑定冲突 |
| 存储 | 实际打开状态、内容根、索引根 |
| 模型与 Prompt | 已配置资源、任务分配、缺项；真实成功调用与 merely configured 分开 |
| 媒体 | 可用解码器及输出格式、FFmpeg/ffprobe 路径、明确缺失能力 |
| 向量与 Wave | 已绑定空间、快照数量/规模、是否已加载 Native、可用通道 |
| 执行 | 根操作数、排队和正在处理的工作；子阶段不冒充新的根工作 |
| 观测 | 日志位置、是否持久保存 Span、导出器状态、实际采样配置 |

最近成功结果从已有 ResultStore 读取必要摘要；error、partial、timeout 等根结果由 `recent_problems` 从同一日志的 `operation.terminal` 读取；返回同一份 issues 和 completed_result_refs。已恢复的 attempt 在操作详情展示，不误当未完成根操作。不以“增加健康账本”为前置。尚未加载的 Native 诚实显示未加载，不为状态检查偷偷建图。

普通 `runtime check` 同时显示近期问题的阶段、错误、已完成引用和操作短号，以及最近一次正文准备的文稿数、候选数和耗时。完整阶段统计由 `--details` 或 `--json` 读取；这些信息来自已有构建日志，不触发重建。

## 根关联与错误

根 operation 在 Execution 入口产生一次稳定关联；子阶段保持 root operation id，另有自己的 Span。共同错误边界把 code/message/stage/attempts/partial refs 和关联附到错误上，CLI 只呈现。

为原生取消/超时附加上下文时复用共同错误辅助值，保持原异常类型到最终入口；不靠逐层扫描 JSON exception notes 恢复业务结果。已有 notes 处理迁移后删除。未进入 Execution 的参数解析错误仍输出统一错误形状，关联可缺省。

一项失败只在负责的最外应用边界记录一次主要错误；重试失败是 attempt 事件，不在每一层复制堆栈。HTTP SDK 日志是辅助信息。

保存成功而请求的标注/索引失败，应用用例通过 [共同操作结果](../foundation/operation-outcomes.md)报告 partial。根日志、runtime logs 与近期问题保留相同错误及完成引用，不因函数正常返回就写成 success。一次 annotate 调用也只有一个完整根关联。

## 事件与 Span

记录 operation、阶段、对象显示名、code、耗时、完成数量/总量（已知时）、结果地址。模型 attempt 另含模型资源、端点协议、序号、状态、切换原因与已报告用量。Native 只在粗粒度边界交付构图/召回/读出诊断；热循环不回调日志。

关闭 `SpanLimits(max_events=0)` 对诊断事件的无条件屏蔽，采用有界事件数/属性长度；开启的事件字段必须能实际输出。export=none 表示不保存完整 Span 树，不能称已完整记录；本地 JSONL 仍保留操作和 attempt 摘要。

实现使用 structlog ConsoleRenderer/ProcessorFormatter 与 OpenTelemetry SDK。终端按明确列展示，JSONL 保留实际 extra 字段。日志字典按各 handler 独立处理，终端隐藏标识不能同时从文件事件删除。

## 例子

```text
媒体描述失败：attachment.bin
阶段：生成描述；模型：vision-main；实际尝试 3 次
原因：上游服务连续返回 503
已完成：原件接纳、JPEG 转换；待完成：本批描述
结果仍可读取：art:… / res:…
诊断：operation …（runtime logs 可查看）
```

排队、实际请求、退避与采用分别计时。日志和错误中不输出 Key、Authorization、整份二进制。取消后也能找到已经采用的阶段。
