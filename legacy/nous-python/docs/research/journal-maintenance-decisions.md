# Journal 维护与检索交付的设计依据

## 材料与结论的范围

项目基线为 `c2944b1675ddb79b9de17beb4f9522349fbbc8ce`。本轮目标沿用已确认的 Journal 生态、程序调用、Prompt 上下文和反馈分工。性能与 partial 的数字/复现情况来自用户独立检查；本包未执行 Nous 测试、CI 或本地性能复测。

项目《VCP 研读》说明了长期几何/传输资产与请求级河网的区别，以及候选质量分、实际排名键和独立来源的语义。本轮保留这套数值资产，修改使用合同、数据准备和正式文稿投影；不以用户报告的良好 Hybrid 结果替代对接缝的检查。

## 关键选择

| 问题 | 选定方案 | 放弃的替代方案及决定性理由 |
|---|---|---|
| 反复 journalize 丢内容 | journalize 只形成新文稿；局部 edits/全文 rewrite/跨篇 reflect 分开 | 给同一压缩 Prompt 增加限制仍要求模型重写和保持整篇文字 |
| 长期修订存储 | 保存完整不可变正文；局部编辑在程序应用 | 读取时串联补丁增加历史读取和来源定位复杂性；目前无需这种存储优化 |
| 共同文稿与后处理 | 事务核心独立，完整用例组织 AfterWrite | 文稿存储逐渐持有所有认知领域会形成中心化大类；事件总线又引入当前不需要的间接性 |
| 保存后查询慢 | 批量描述、一次解析、准备结果复用；仍发布完整快照 | 当前 Native 仅约 0.15 秒，先换算法或搭后台队列掩盖的是重复准备 |
| 长文占名额 | Native 召回和交付使用正式分组 | top-k 乘更大常数不能保证取得其它文稿，前端补查又重做观测和排名 |
| partial 难追查 | 根操作显式业务结果，所有诊断消费同一终态 | 某个 CLI 手写日志无法服务程序调用，也不能改变 runtime check |
| Prompt 可编辑性 | TOML 说明＋从真实任务生成技术上下文 | 手写另一份 Schema 或 YAML Front Matter 会形成两个权威 |
| 查询效用 | 类型化上下文与目标，明确反馈时保留 | 对 Journal 加全局好坏权重会混淆情境相关性、准确性和不同意 |

## 外部资料与工程使用

### 精确选区

[W3C Web Annotation Data Model](https://www.w3.org/TR/annotation-model/) 同时表达文本位置、引文以及资源状态，并支持在已有选区内进一步定位。本轮借用其“版本＋选区＋原文核对”的区分：程序中的 UTF-8 坐标属于 Nous 自己的合同，模型使用块键和精确引文。没有照搬一套 RDF 或复杂标注服务器。

### 分组检索

[Tantivy Collector](https://docs.rs/tantivy/latest/tantivy/collector/trait.Collector.html) 提供分段收集和归并接口，适合实现本项目的组内 top passages。段访问无固定顺序，所以应按得分归并，不按首次遇见占位。[TopDocs](https://docs.rs/tantivy/latest/tantivy/collector/struct.TopDocs.html) 的候选 top-k 本身不提供本文稿分组语义。

实施使用 Cargo.lock 里的依赖 API，不因文档页面展示了更新版本就全仓升级。向量通道继续使用现有 USearch 过滤能力；本轮增加的是共同组收集政策，不重写 ANN。

### 操作结果

[OpenTelemetry Recording Errors](https://opentelemetry.io/docs/specs/semconv/general/recording-errors/) 为错误、状态和事件提供公共语义。Nous 的 partial 是业务结果；采用现有 Span 属性/事件和 ERROR 状态来表达明确请求未全部完成，使用共同 JSONL 保存详情。恢复成功的 attempt 不应让根永远失败。

### 成熟基础

[Python difflib](https://docs.python.org/3/library/difflib.html) 用于读差异，不执行模糊修改；[Jinja API](https://jinja.palletsprojects.com/en/stable/api/) 继续负责模板加载、StrictUndefined 与编译缓存。现有 SQLAlchemy 批量查询/事务、MarkdownIt 源映射和 CAS 保持负责位置，不自建替代框架。

### 后续持久执行

[DBOS Client](https://docs.dbos.dev/python/reference/client) 当前支持配置实例方法的 class_name/instance_name，并提供同系统数据库的事务入队；[Workflows](https://docs.dbos.dev/python/tutorials/workflow-tutorial) 描述恢复和步骤记录。下一轮应按实际所用版本固定这些接口。此资料支持下一步方案，不表示本轮已加入 DBOS。

### 情境性反馈

[Explicit In-Situ User Feedback](https://www.microsoft.com/en-us/research/publication/explicit-situ-user-feedback-web-search-results/) 讨论实际搜索情境中的评价；[查询反馈重排研究](https://aclanthology.org/2022.emnlp-main.614/) 提供查询级反馈利用的方法参考。[LongMemEval](https://arxiv.org/abs/2410.10813) 区分知识更新、时间等长期记忆能力。它们支持保留语境和设计有区分力的实验，不保证某种方法在 Nous 必然改进。

## 本轮保持的研究边界

派生 Journal 的 interpretation 贡献是 Nous 对输入结构的扩展，不能声称来自所有上游 VCP 的相同定义。程序可以保证位置、版本、来源和组资格；模型提出的概念路径仍可能有语义噪声。通过原文、可读来源和明确实验观察它的效果，不增加层层模型投票。

还需后续研究的内容是：WorkContext 中效用如何调整本次检索，Concept 语义候选如何改善身份复用，Episode 时间结构如何决定历史视图，以及持久维护怎样在同一宿主内恢复。它们不阻塞本轮已经明确的 Journal 维护和共同能力整改。

## 与基线实现的具体连接

本轮输入材料包括用户对 c2944b1 的独立测量，以及该提交中的 `cognition/records.py::complete_save`、`cognition/retrieval.py`、`wave/assets.py::freeze_inputs`、`content/projections.py::semantic_text`、`observability/diagnostics.py` 与 Native query/lexical 实现。局部结果通过私有字段重组、候选重复准备和末端文稿去重是本轮改造位置；用户的数值保持为该机器实测，不延伸为其它设备的性能结论。

基线源码可从 [c2944b1](https://github.com/Heptalogos-Devs/Nous/tree/c2944b1675ddb79b9de17beb4f9522349fbbc8ce) 读取。开发以现行工作树为实际对象，基线之后的有效修复需要合并而非覆盖。
