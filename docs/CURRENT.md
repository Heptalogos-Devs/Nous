# 当前工作

## 已有能力

本地实例、配置、启停、SQLite、CAS、多轴时间、执行和观测保持共同入口。当前数据库 schema 为 4，表由组合根显式收集；已有其它 schema 的实例不自动改写。

模型和媒体共用输入规划、冻结任务与请求尝试策略。普通任务联合处理多份材料，独立逐批执行显式选择。每次只准备实际选中的路线，备用路线在真正切换时才按同组原件和选区准备表示及所需转写；媒体分析各阶段也通过共同生成操作选择路线。媒体按真实类型协商表示，转换在消费当前批次时执行，保存实际 MIME、范围和来源。Prompt 预览、probe、生成与媒体各阶段消费同一套能力，纯预览不发语言请求。

Embedding 的 run、ensure、probe 和临时查询使用同一个批次执行器。已采用向量依空间、用途和精确输入选择规范行；网络不占用复用长锁，少量行读取通过 CAS 本地借用及 NPY mmap。临时查询默认不接纳新的正文或向量结果。

内容关系按输入组、part、origin 和输出规范化保存。来源读取只遍历目标相关祖先；TextView 以 CommonMark 源映射提供精确块与范围，模型选段、概念标注和 Wave 共用。

根操作保持稳定关联，子阶段和实际请求尝试可通过 JSONL 追踪。`runtime check` 报告实际配置、资源、已采用结果及 Native 加载状态；`runtime logs --operation` 读取保留日志。默认不持久保存完整 Span 树。

Wave 使用 Rust retrieval 的 Engine、冻结快照、观测、完整 CandidatePool、召回与读出；PyO3 负责数组、detach 和对象边界。VCP 数值基准为 `d5c019e`，具名 DTSC 保留。稀疏图、长出现序列、多路径和明确支持组均有实际消费；查询可使用临时编码或已采用的显式 VectorRef。

`journal-draft` 和 `annotate-concepts` 生成普通 Markdown/标注 ProcessingResult。真实正文、概念出现和路径可进入 Wave，结果能按原文范围回读；候选范围完整覆盖的出现项独立形成概念归属，空 paths 保留概念匹配和直接锚，原文回读同时给出冻结的概念与路径。没有创建正式 Journal 或长期 Concept。

## 有效决定

保持供应商中立，真实模型访问统一网关。Python 拥有正式状态和业务采用，Rust 拥有可重建计算与索引。媒体、向量和 Wave 继续复用配置、Execution、事务、时间、内容与观测 owner。

血缘根集合不等于独立支持。关系通过 SupportGroup 区分共同前提与独立见证，解释性贡献通过显式 profile 启用，检索和标注不自动改变长期认识。

## 后续范围

完整 Journal CRUD、自动长期采用、修订生命周期、时间门控执行、DBOS、HTTP/MCP、远程 SDK 和完整 NousQL 尚未接入。Journal/时间视图设计保留为后续合同，当前实验不承担这些生命周期。
