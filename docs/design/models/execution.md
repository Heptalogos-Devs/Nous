# 模型执行与成功结果采用

## 分工

ModelExecutor 拥有真实协议请求、响应结构、完成状态、流、有限尝试、取消和用量。GenerationOperations 拥有完整语言任务以及结果采用。GenerationOperations.transcribe 组织标准转写阶段并采用成功结果。模型执行不查询 CAS、业务 SQLite 或媒体准备器。

统一连接与凭据继续复用；协议转换采用 Pydantic AI Direct API 和官方 SDK。New API 只是开发网关，生产代码不依据厂商名、模型名字或本地容器写分支。

## 准备与执行值

```text
FrozenTask = 任务定义 + 模板内容快照 + 任务配置 + 有序候选路线
PreparedRun = FrozenTask + 有序 PreparedBatch 引用计划
PreparedGeneration = FrozenTask + 单批实际消息 + 输出合同 + 合法路线
execute(PreparedGeneration, context) -> GenerationResult
stream(PreparedGeneration, context) -> async context manager[GenerationStream]
ModelExecutor.transcribe(task, audio_bytes, filename, snapshot, context) -> GenerationResult
```

阶段语义固定，候选路线的协议参数分别准备。PreparedRun 持有 CAS 引用；应用在当前批次前物化媒体并编译 PreparedGeneration，完成后释放。一次根操作固定其模板，重试/fallback 不读取修改后的模板。

ModelRegistry 负责解析 task/model/preset 和主次路线。Probe 使用显式模型选择建立一次任务，不伪造 ConfigSnapshot 或 tasks 条目。普通配置可读/状态检查不发请求。

## 执行策略

生成、转写、Embedding 复用同一组截止时间、可重试错误分类、退避和尝试统计；各端点仍封装自己的请求和响应。把已有共用函数移到窄的请求策略模块，不通过导入整个 generation executor 来复用一小段逻辑，也不为共用循环发明自主工作流。

SDK 自动重试为零或明确纳入同一计数。只有网络/服务暂态失败和显式启用的结构修复进入尝试。输入、模板、能力或来源错误直接返回。保存失败发生在尝试循环之外。

## 完成和流

保留当前严格流完成条件：Chat Completions 必须有明确 finish_reason，SDK response.state 必须 complete，length/filter/error 返回实际失败。不能通过 compatibility 配置关闭该条件。EOF 或 `[DONE]` 本身不能代替完成响应。

首个可见 delta 之前可走符合合同的备用路线；之后输出来源固定，失败交付部分输出。只有响应完成、任务校验并成功采用后，CLI 才发 completed 与正式 result ref。取消原样传播并关闭实际响应。

## 实际请求及用量

ResultStore 保存语义消息、actual model/provider/protocol、模板及 Schema 版本、actual attempt 参数、输入 part refs、生成时间、结束原因和可用请求 ID。结构修复追加的响应与反馈也进入本次实际请求记录，不能只保存初次 preview 却宣称它是最后请求。

请求参数必须是经过合并并实际交给 SDK 的参数；不只记录原 preset。已返回用量累加；未报告的 token、费用保持未知。采用者负责正式数据生命周期，诊断 Trace 不承担唯一来源记录。

## 封装与迁移

Executor 移除 extract-information / passage-selection 的专名分支，由任务合同执行；GenerationOperations 通过任务 outputs 采用。MediaPreparer 移除转写，MediaAnalysis 通过 GenerationOperations.transcribe 组织需要的阶段。`GenerationOperations.run` 消费已经准备的 ReadingInput 与冻结任务；`run_prepared` 执行已编译请求并采用。

## 联合输入与实际尝试

任务的 joint/independent_batches 及共同材料规划见 [输入规划](input-planning.md)。多个 input 的原数组定义一次任务，不能在准备时无条件变成“一文件一次生成”。现有 PreparedRun 持有共同计划，按 TaskDefinition 的语义形成实际请求。

路线准备、修复、实际请求参数与诊断统一遵循 [请求尝试](request-attempts.md)。PreparedAttempt 在发送前形成，show-request 与采用记录使用同一对象。Generation/转写/Embedding 复用 policy 的尝试上下文。
