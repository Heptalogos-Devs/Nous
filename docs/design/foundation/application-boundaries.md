# 完整应用操作与共同输入边界

## 所有权调整

应用层负责一个用户动作所需的跨 owner 协作，底层 owner 封装本领域的完整机制。保持已有 ConfigurationOperations；GenerationOperations、MediaAnalysis、EmbeddingOperations 与新 WaveOperations 各自负责实际成果。

共同复用的能力按已有职责提取：

| 能力 | 定义位置及责任 |
|---|---|
| InputSpec、ContentSelection、InputGroup | content/contracts；同一输入合同用于模型、媒体、Embedding、Wave |
| 接纳本地/内联/已有引用 | ArtifactStore 的完整接纳操作，不在 CLI 或每个消费者重写 |
| 原文块与精确片段读取 | content 的文本读取能力，使用 markdown-it 与不可变 Artifact |
| 具体格式的准备与批次规划 | media；PDF 确定性抽文也供 Wave 使用，没有语言模型依赖 |
| 任务语义、来源键校验与产物解释 | models/tasks 中静态任务定义 |
| 模型尝试策略 | models 中小型共同执行策略，被生成/转写/Embedding 使用 |
| 操作范围内 worker 生命周期 | Execution，CAS 和 Native 使用同一机制 |
| 地址编码与解析 | common 的纯值能力；具体 owner 在自己的表内分配并处理唯一约束 |

这些是职责，允许相邻的内聚函数同处一文件。不要为每个动作创建 repository/service/manager 三层转发。

## 应用接口

```text
GenerationOperations.run_input(task, InputSpec[], snapshot, options) -> GenerationOutcome
GenerationOperations.preview(prompt, inputs, snapshot, options) -> prepared previews
GenerationOperations.probe(model, input?, snapshot) -> GenerationOutcome
GenerationOperations.run(FrozenTask, ReadingInput, snapshot, context) -> ProcessingResult
GenerationOperations.transcribe(source, audio_part, snapshot, context) -> ProcessingResult
MediaAnalysis.analyze(AnalysisRequest) -> AnalysisOutcome
EmbeddingOperations.ensure(EmbeddingRequest) -> EmbeddingBatchRef
WaveOperations.build(CollectionManifest) -> WaveSnapshotRef
WaveOperations.query(WaveQuery) -> WaveResult
```

InputSpec 是产品输入值，context 由根操作建立并向内部阶段传递。普通入口使用已打开 Runtime 的 snapshot，不复制配置或伪造版本。内部阶段显式接收冻结任务/输入，与现有根上下文配合。

`InputSpec` 是 File(path)、InlineText(text, display_name)、Existing(ArtifactRef, selection) 三选一。有序数组由应用保持；CLI 仅转换命令参数。文件接纳之后一律引用 CAS 的已保存字节，原文件变化不改变本次内容。

## 预览与探测

纯 Prompt 预览调用共同输入/编译能力，说明使用的是模板级表示。指定实际 task 的预览则采用该任务的媒体与输出合同。两者都不偷偷运行转写或描述。

`--show-request` 使用与本次执行相同的已准备消息。probe 接收显式模型选择和真实输入，调用正式准备/执行路径；其用途作为调用参数，不通过篡改全局 tasks 配置或伪造配置版本实现。

## 结果和阶段

每个成功语言阶段采用一次 ResultStore。多阶段应用返回根操作状态、成功阶段 refs 和未完成范围。确定性缓存复用同一真实转换结果；LLM 结果复用必须显式选择且核对输入、任务、表示和范围。

模型执行器只判断协议响应、完成状态和输出形状。信息抽取的 source_keys、原文选段恢复、未来概念描述等都由对应任务合同负责。新增语义任务无需修改 Executor 中的任务名分支。

## 同轮迁移

移除 MediaPreparer 的 ModelExecutor 参数；移出 `_audio_source` 的转写和采用。迁移 CLI 中 probe 临时配置与 preview 的硬编码 MIME/新 Compiler。移出 Executor 的具体任务判定，迁移真实消费者并删除旧入口。当前可用的流结束、错误映射、选区及缓存修复继续由各 owner 承担。
