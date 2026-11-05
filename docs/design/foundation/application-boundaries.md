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

共用短地址由类型前缀、冒号与六个以空格分隔的四字母单元组成。首单元从固定的 1,024 项表均匀选择，其余各从固定的 2,048 项表均匀选择，共 65 位随机空间；单元表及其顺序固定。写入由所属表的唯一约束处理碰撞，失败时重新分配。地址按完整字符串传递，在命令行中使用时须整体加引号。

## 应用接口

| 使用目的 | 公共服务与调用契约 |
|---|---|
| 多输入语言生成、预览与探测 | [GenerationOperations](../../../src/nous/models/operations.py)：run_input、preview、probe |
| 成功计算与独立结果采用 | GenerationOperations.produce / adopt_produced；成功计算的保存恢复不重复请求模型 |
| 多媒体解释 | [MediaAnalysis.analyze](../../../src/nous/media/analysis.py) |
| 文本向量复用与查询编码 | [EmbeddingOperations.ensure / encode_query](../../../src/nous/application/embedding.py) |
| 集合构建、查询与原文读取 | [WaveOperations](../../../src/nous/wave/operations.py)：build、build_view、query、read |

各服务的准确参数、返回值、异常与资源限制写在源码 docstring 中。跨服务的正式认知操作见[进程内认知入口](../interfaces/cognition-api.md)。

InputSpec 是产品输入值，context 由根操作建立并向内部阶段传递。普通入口使用已打开 Runtime 的 snapshot，不复制配置或伪造版本。内部阶段显式接收冻结任务/输入，与现有根上下文配合。

`InputSpec` 是 File(path)、InlineText(text, display_name)、Existing(ArtifactRef, selection) 三选一。有序数组由应用保持；CLI 仅转换命令参数。文件接纳之后一律引用 CAS 的已保存字节，原文件变化不改变本次内容。

## 预览与探测

纯 Prompt 预览调用共同输入/编译能力，说明使用的是模板级表示。指定实际 task 的预览则采用该任务的媒体与输出合同。两者都不偷偷运行转写或描述。

`--show-request` 使用与本次执行相同的已准备消息。probe 接收显式模型选择和真实输入，调用正式准备/执行路径；其用途作为调用参数，不通过篡改全局 tasks 配置或伪造配置版本实现。

## 结果和阶段

每个成功语言阶段采用一次 ResultStore。多阶段应用返回根操作状态、成功阶段 refs 和未完成范围。确定性缓存复用同一真实转换结果；LLM 结果复用必须显式选择且核对输入、任务、表示和范围。

模型执行器只判断协议响应、完成状态和输出形状。信息抽取的 source_keys、原文选段恢复、未来概念描述等都由对应任务合同负责。新增语义任务无需修改 Executor 中的任务名分支。

