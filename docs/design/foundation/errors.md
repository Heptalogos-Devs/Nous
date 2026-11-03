# 错误表达与异常传播

## 目标

错误让调用者知道哪项操作未完成、原因在哪里及可采取的下一步。日志、CLI、HTTP、MCP 和 SDK 使用共同错误数据，各自负责呈现。错误基础不依赖数据库、配置或 Trace 服务。

## 共同数据

`NousError` 表达稳定 `code`、本次操作名称、用户可读 message、结构化 details 和原始 cause。必要时包含 `retryable`、`retry_after`、`operation_id`；只有已经产生 Trace 时才包含 trace_id。

代码按所属能力命名，例如 `configuration.invalid_value`、`configuration.binding_conflict`、`instance.in_use`、`storage.unavailable`、`time.domain_mismatch`、`model.unconfigured`、`operation.cancelled`。错误码按有意义的类别建立，不为每个私有 helper 分配编码。

Validation details 可以包含字段路径、期望类型、实际输入的安全摘要和来源位置；凭据、授权头及完整模型正文不自动进入错误。

## 传播规则

底层保留库异常。只有能够补充稳定业务含义的负责边界才将其转换为 NousError，并使用异常链保留原因。中间纯转发层直接传播，不逐层包装同义错误。

异步取消保持原有取消语义，不能被通用重试捕获。超时明确属于哪次操作及已完成部分。模型部分输出后中断的结果由模型执行合同表达，不返回普通“成功但空文本”。

最终入口负责一次主要错误日志和用户结果。深层只在记录独立诊断事件有价值时写日志，避免每层重复完整堆栈。

## 呈现与重试

CLI 默认给出简洁原因及下一项可执行操作，`--debug` 展开堆栈，`--json` 输出结构化错误。首轮退出码：0 成功，2 输入/配置问题，3 当前资源或能力不可用，4 操作失败；Ctrl-C 使用平台自然中断语义。

HTTP/MCP 将来映射相同错误数据，不让领域依赖 HTTP 状态码。`retryable` 只提供分类信息，具体重试由负责该操作的执行器决定，不在每一层自动重试。

## 完成条件

同一无效配置通过 CLI 和进程内调用获得相同 code 和字段信息；原始 cause 可在调试中定位；错误格式化本身在配置和数据库不可用时仍正常；一次失败没有多份重复堆栈。
