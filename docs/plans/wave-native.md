# 结构收敛与 Wave Native 实施计划

## 最终交付

在当前真实 Nous 上完成两项相连的成果：整理模型/媒体/内容/配置的职责接缝；将完整 VCP 数值能力和 Nous 检索扩展接入 Rust Native，提供 build → query → explain/think → read → reopen 的正式通路。

基准为 fdb39e4，开工读取最新文件。先读架构、application-boundaries、content-records、wave/overview、assets、contracts、native、vcp 和研究对照。用户补充的有效边界要求合入相应当前专题。审查者不运行本地测试或读取 CI；用户独立验证。

## 真实职责迁移

| 当前位置 | 本轮修改 |
|---|---|
| media/preparation.py | 移出模型转写；批次变成引用计划；模态内部缓存与转换按职责组织 |
| media/analysis.py | 完整组织转写/描述/结构化，冻结任务，逐批采用与返回部分结果 |
| models/operations.py | 共同接纳/准备、preview/probe、语言阶段与任务采用 |
| models/executor.py | 保留协议、完成状态、尝试与流；移出专名任务判断与共享策略 |
| models/prompts.py | 资源快照、单一 Compiler、编辑/预览合同 |
| content/contracts.py、store.py | 输入组/实际 part 与 origin 分开，精确读取与批量来源能力 |
| models/embedding*、application/embedding.py | 批量 exact ensure/read 和空间行映射，供 Wave 使用 |
| network、configuration、application/configuration.py | 凭据重启语义、缺失/null、配置元信息与单一采用 |
| runtime/execution.py、storage/content.py | 共用阻塞取消/等待实际完成，borrow_local 与既有导出修复 |
| wave/ | 新增集合、冻结资产、查询规划、Native 消费和结果读取 |
| native/vcp、retrieval、python | 纯计算、索引、PyO3；吸收旧解耦成果和最新上游计算 |
| transports/cli | 保留产品动作，删除入口内业务编排，接入 wave |

源码组织以实际深模块为准，不按文档个数机械建类。清理覆盖实际生产者、消费者、schema、配置和当前指南，删除旧路径，不保留 alias/桥接来通过旧测试。

## 先收敛可复用基础

先完成 InputGroup/part/origin 与 TaskDefinition。迁移现有生成、媒体和结果读取，使每个实际发送表示有精确正文归属。模型接口保持真实流、结构化与错误能力，已有选区/PDF缓存/执行关闭修复继续成立。

共用输入、Task/Prompt 冻结、按需物化和 ConfigurationOperations 决定后，CLI 回到单次应用调用。不要等 Wave 加完才修补来源形状；Wave 必须从第一天消费正确输入。

## 并行但有明确依赖的开发

基础收敛时，可以独立开展 Native 计算迁入：以冻结数组合同为共同输入，计算开发不等待认知对象。但 Python Wave 接入必须消费已收敛的内容/向量 owner。

Native 维护者负责算法与 schema，Python Wave 维护者负责集合/查询/来源映射，模型媒体负责人完成已有能力收敛。共享输入值与事务由清楚的一方定义，其他人直接迁移调用方，不各自复制 DTO。

## Native 迁入

以 legacy/native 中独立 crate 为起点，对照 VCP d5c019e 更新完整计算闭包，包含新的 rivermemo_wormhole 及 quality/ranking 分离。迁移 AGS、资产、Pyramid、门控、Sense、双场、曲线/拓扑/Ω、Thinking、原候选通道与去重热点。

去掉上游 SQL/N-API/全局运行壳；用 Typed FrozenAssets 替换输入读取。保留旧 Nous 多根来源修正；DTSC 作为独立具名模式从历史基线提取，其它 obsolete baseline/兼容门面不随整体恢复。

PyO3 build/open/query/observe/readout/think 使用同一数值实现。数组进入 Native 后具有实际所有权，detach 期间无 Python 对象访问；配置 schema/defaults 同源导出。持续复用成熟 ANN、全文索引与矩阵依赖。

## Wave 应用通路

使用本包例子或用户真实资料接纳集合。Concept 完整描述、候选正文和 query/context 经 EmbeddingOperations.ensure 得到同一空间实际向量。按根来源只构建一份事实图，保存候选到多根来源与有序曲线。

构建发布不可变快照，登记短地址。query 选择合法候选、上下文与 cues，Native 产生真实候选和解释；read 从内容 owner 读取原文。重开不重新调用文稿/Concept embedding。没有模型时 lexical 与已存内容可用。

Typed associations、多根来源、上下文、时间过滤和反证读取须有实际消费者。不能只加字段、导出 schema 或留注释而没有对应行为。没有正式认知对象时清单是明确的实验输入，不伪装成主体自动学会的认识。

## 重要完成行为

已有模型/媒体通路在职责迁移后仍可使用；preview 无隐式模型调用，结构化和流的真实完成条件保留。长 PDF/视频按批次物化，不随总页数保存全量媒体 bytes。

Native 实际运行 lexical、dense、tagmemo、rivermemo、hybrid、dtsc；完整 Thinking 使用同次非截断 transitions。多来源候选不重复自证，描述型 Concept 与 unknown cue 真正进入算法。质量分、排名键和最终 rank 一致使用各自合同。

停止或取消时 Native worker 退出后释放资源；build失败不切换 current；重开加载同一快照。无事件循环阻塞的逐条计算回调、无业务 SQL 进入 Rust、无第二套向量客户端。

## 验证与交付

采用 [实际操作说明](../development/wave-walkthrough.md) 中能改变判断的通路。实现者可运行真实命令调试发生的故障；用户独立全面验证。沿用必要高价值数值回归场景，删除只保护旧结构的测试；不建逐模块测试平台、不增加 CI，不按通过率宣布架构正确。

记录实际运行的模式、输入规模、结果、限制和未接通点。没有测得的性能不写成提升数字。源码索引、AGENTS 中受影响的接入点、负责 design 及 CURRENT 原地更新；不新增过程账本。

完成即停止。DBOS、完整 NousQL、Note/Journal/Social/Ego 生命周期和新传输入口不随本轮自动展开。网关补丁和用户样本生成工具不改。
