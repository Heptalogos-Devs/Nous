# Wave 查询程序

Wave 把一次查询表示为带类型的算子图。配置 `wave.default_program` 选择普通查询使用的程序，`wave.programs.<name>` 定义研发配方；`wave.unavailable_program` 指定默认程序缺少快照能力时使用的程序。显式选择的程序缺少能力时返回错误，显式探索也要求程序声明 thinking 输出。当前默认 dense 使用原问题向量召回，并在有明确检索情境时加入独立的情境词项分支；无向量时使用 text_only。

每个 `WaveProgram` 包含 `nodes` 和 `outputs`。节点有稳定 `id`、`operator`、按端口连接的 `inputs` 和 `parameters`；引用为 `{ node, port }`，多值输入是引用数组。内建 `query` 提供 `intent`、`raw_vector`、`contextual_signal`、`lexical` 和 `stages`，`snapshot` 提供 `handle`，`scope` 提供 `allowed`、`materials` 和显式固定候选，`delivery` 提供请求。

`compile_program` 验证算子、端口、类型、依赖、环和参数，按拓扑排序；输入、参数完全一致的确定性节点只执行一次。数值算子参数 schema 与默认由 Rust 导出。程序需要的 text、vectors 和 vcp 能力从实际节点计算，查询固定可用快照后校验。程序内容签名和实际执行名称进入查询摘要。

Native 在一次快照借用中执行数值子图到局部定位。观测、CandidateSet、CalibratedPool 和 RankedSet 在该借用内共享，RiverMemo 消费绑定同一观测的校准池，DTSC 消费明确候选集。Python 将行映射为原文范围，按图执行可选模型评价和预算选择；模型工作使用共同的评价 owner。结果保留原问题、实际程序、评分尺度、组排名与局部片段。

每次执行返回 `ProgramRunFacts`：实际程序、选择来源、参与节点及其默认值已展开的参数、相关运行设置、预算和空间绑定。`definition_signature` 标识声明配方，`signature` 标识本次有效执行设置；两者都不参与资产构建身份。该事实随 EvidenceDelivery 保存，完整 trace 是独立可选项。固定候选与表达式的使用状态在执行摘要中标明，表达式内容和约束留在 QueryIntent。

`passages.evaluate` 的 `method` 选择 `rank` 或 `evaluate`，`policy` 选择失败时继续还是中断；`task` 指向共同模型任务。评价可声明 `purpose`、`rubric` 和 `scale`。数值评价通过 `numeric_order` 选择 `higher` 或 `lower` 优先；分类量表只约束允许值，只有另行填写覆盖全部类别的 `category_order` 才按给定次序选择，省略时保留判断而不排序。

内置 `lexical`、`dense`、`tagmemo`、`rivermemo`、`hybrid` 和 `dtsc` 提供可比较的研发配方；`research` 展示同次观测、多路候选与 RiverMemo 的组合，`fixed_set` 消费调用者提供的快照候选用于同池对照。普通应用操作使用同一配置入口，不自行映射算法名称。

程序节点、端口及数学次序见 [算子](operators.md)；局部证据与预算见 [原文定位](localization.md)。
