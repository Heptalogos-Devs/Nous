# 模型请求准备、尝试与诊断

## 唯一尝试所有者

`models/policy.py` 管理共同的期限计算、错误分类、退避和 attempt scope；生成、转写、Embedding 的端点执行者负责各自 SDK 调用和响应解析。使用已有依赖和统一网关，不增加厂商分支、第二层协议客户端或通用自主工作流。

```text
PreparedAttempt:
  route/resource
  immutable semantic input and actual representation refs
  sdk_messages_or_payload
  merged protocol parameters
  limits {output, transport, deadline}
  attempt_number

AttemptReport:
  number, route, start/end/duration
  outcome, error_code?, provider_status?, provider_request_id?
  retry_reason?, next_action?, reported_usage?
```

AttemptReport 是本次操作记录，成功结果随 ProcessingResult 保存必要信息，失败随错误/日志保存。它不是需要调度的持久任务实体。

## 参数与切换

冻结任务包含模板内容与配置快照。每条路线的输出 token、上下文和参数独立计算。PreparedAttempt 是实际交给 SDK 的值；show-request 显示它，存储也保存它。已知输入超限或格式不兼容只使该路线不可用，不能提前把主模型限制缩成全链最小值。

当前上游返回结构失败且显式允许修复时，仍用同一路线附加具体反馈；服务暂态错误才切到下一路线。取消、调用方错误、模板错误、空间错误、保存失败不进入重试。所有真实提交共用该语义阶段的 max_attempts，SDK 自身不另行重试。根 deadline 包含等待及退避。

PreparedGeneration 只持有当前路线的已编译输入，后继路线尚未准备。切换时，executor 调用应用提供的 prepare_route 回调，由共同输入操作物化新表示及必要的已配置转写。当前路线的能力或表示不足可以跳过，不产生虚构的聊天请求尝试。转写实际调用有自己的 task/attempt；暂态失败影响所选路线时，原因记入该路线的 ineligible_routes，preparation_route 标明归属，然后继续选择后继路线。

流一旦交付可见文本，后续失败保持同一来源；不拼接新模型的答案。最终完成记录要求协议响应真实完整且结果采用成功。保留当前实际 finish_reason/state 检查，不以 EOF 代替完成。

## 输入量

文本可使用库提供的 tokenizer；没有可靠模型计数时标记估计并记录算法。未知媒体 token 保持未知。传输字节、图片尺寸、音频时长、帧数和实际模型窗口是不同合同，不使用压缩 bytes/256 的任意公式硬性拒绝正常媒体。配置的技术上限可以执行，说明其实际单位。

## 关联与观测

从 Execution 取得同一个根 operation_id、已存在的 Trace context 和本次 stage。每个网络尝试使用一个子 Span，记录任务/模型资源/协议、尝试序号、耗时、实际状态、错误 code 和切换理由。重试成功也保留先前失败。没有 Trace 时同样输出结构化 attempt 日志。

CLI 不添加网络埋点；MediaAnalysis 和 Wave 只表达自身阶段，不另记一套 attempts。Provider SDK/HTTP 日志只作补充。

## 用量与结果

累加已知实际用量，缺失项保持 unknown/partial。未执行的备用路线不产生虚构用量。记录资源 ID、实际返回模型名和 Provider request id，不采集凭据。默认不将完整 Prompt/模型正文放入普通日志；正式产物仍由 ResultStore 保存。

失败经过共同错误边界保留 operation_id、trace_id（存在时）、stage、code、attempts 和 completed_result_refs。保留原异常链，不逐层重新包装同义错误。取消的 Python 语义保持到最终入口。
