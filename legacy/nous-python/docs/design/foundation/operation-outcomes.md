# 完整操作的部分完成与诊断

## 一份业务结果贯穿所有出口

共同操作必须表达 `success | partial | error | cancelled | timeout`。没有异常离开函数不能单独决定成功；调用者收到 partial 后，操作日志和 runtime check 必须能找到相同失败原因。

`common/operations.py` 提供纯值与明确报告入口：

```text
OperationIssue:
  stage, code, message
  details, retryable
  affected_refs[], completed_refs[]

report_outcome(status="success"|"partial", *, issues=(), completed_refs=())
```

该方法使用现有 OperationDiagnostics/OperationFrame，不依赖 OTel SDK、CLI 或 cognition。它不把 arbitrary 返回字典自动猜成业务结果。完整用例在确定响应时报告；共同 instrumentation 在根操作退出时统一生成终态日志与指标。`record_completed` 继续只登记已提交引用。

异常逃逸的 error/cancelled/timeout 优先于显式 partial；其余终态由已报告业务结果决定。嵌套 scope 的 success 不覆盖根的 partial。重试成功只保留 attempt 失败记录，最终没有未完成要求时仍为 success；不因任何一次网络失败都把根标为 partial。

## 先提交、后维护

RecordOperations 的主保存提交成功，启用的自动标注失败时：

```text
response: status=partial, saved revision=..., annotation=failed
root log: result=partial, issues=[model.task_unconfigured/...], completed refs=[revision]
runtime logs: result=partial，错误 code/message/stage 可以读取
runtime check: 最近问题包含该操作及已保存内容
```

正常 defer 返回检索 not_requested，结果是 success。保存前验证失败没有已保存内容；标注后索引失败只附相应问题，不能把已经采用的标注标成失败。

取消保持 Python 原异常语义，附已提交引用与当前阶段；程序中止不会靠 catch 后返回 success 伪装完成。

## 日志、Trace 和指标

保留现有 logging/structlog/OpenTelemetry 装配。每个根操作产生一份 `operation.terminal` 事件；日志读取、近期问题和终端输出都消费该事件。阶段和 attempt 事件仍用于展开过程。

终态事件至少包含 operation_id、operation、result、duration、completed refs、issues。普通终端显示业务名称、对象、部分完成说明和短诊断引用，完整 ID 留在 JSONL。partial 使用 warning 级别。不会把嵌套 stage.completed 当成根最终结果。

OTel 根 Span 记录 `nous.outcome=partial` 与相应 issue 事件；本次明确请求未完成时根状态设 ERROR，成功的 commit 子阶段仍为成功。纯 attempt 恢复成功的根不记为 ERROR。沿 SDK 原有 Status 与事件机制实现，不创造新的 OTel status。

operation.count/duration 的 result 标签同时包含 partial，数量不依赖 response 是否抛异常。无需为每个 Note 建指标标签或把 UUID 当 metric attribute。

## 日志读取和近期问题

`observability/diagnostics.py` 从现有轮转 JSONL 找该根的 `operation.terminal` event，返回 result、issues、attempts、已完成引用和事件。文件有后续 stage 日志也不改变根结论。没有终态时报告 incomplete/unknown，不能用最后一条局部 success 猜根完成。

共同 `recent_problems`：优先根的 error/partial/timeout，合并同一次操作；成功根中的已恢复 attempt 可以在该操作详情看到，不挤占“近期未完成问题”。读取仍使用原日志保留策略，不增加 SQLite 故障账本。

诊断丢失、日志关闭或轮转清理时明确说明可用范围，不伪造历史结果。完整 Trace exporter 未配置时继续说明只有日志关联，partial 的定位不能依赖外部 exporter。

## 接入调用者

- RecordOperations 对保存后的标注/刷新报告一次真实业务结果。
- MediaAnalysis 对已完成批次与未完成覆盖使用相同值；主失败仍抛现行错误。
- WritingOperations 对已存草稿、采用失败保留草稿和失败阶段。
- CLI annotate 调用完整 AnnotationOperations.annotate；显式 draft-only 才调用提案模式。CLI 不自行拼 propose/adopt，因此只有一个根关联。
- 所有新使用者通过同一个公共结果/错误机制接入，不在 CLI 手工打印一条“partial日志”。

错误完整 code/message/details 由负责 owner 提供，展示翻译只改变界面文案。共同 attempt 记录实际 next_action：retry_same_route / switch_route / repair_same_route / raise。

## 部分失败只接纳已知的阶段错误

提交后的标注/刷新可将负责者已经分类的 NousError 表达为 partial，保留 code/message/原因链及所有已提交引用。未预期的程序错误、断言失败和未分类异常继续离开操作并成为 error；不得 `except Exception` 后统一包装成可忽略的 partial。取消和根期限超时继续抛出原类型。

同一类提交后维护只在 RecordOperations 的共同过程处理，Feedback 和 DraftStore 不再复制 try/catch。内层只提供错误及其发生阶段。`report_outcome` 能把已确定问题带到根；它不把任意内层尝试失败永久升级为根失败。

终态的 issue code 与返回值使用同一对象内容。终态 partial 即使没有抛异常也应出现在 `runtime logs` 的错误/问题区域和 `runtime check` 的最近问题；不能只更新终端显示而遗漏日志 reader。
