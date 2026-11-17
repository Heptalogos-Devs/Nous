# Journal 首条使用链的研究与选择

## 资料与决定分开

项目基线为 Nous `4403681034eb24ed5d40cf3062bb76010ce86fe4`；上游算法沿已采用的 VCP `d5c019ebbac1f317c2938d655d2ce916e41b9164`。本轮不重新选择 VCP 数学公式，也不以外部论文取代项目的文稿、视角、来源和多轴时间语义。

本轮工程决定：保存可读原文与不可变修订；概念标注独立采用；正文与检索发布分开；当前查询固定一次已提交视图；模型只写作/标注；现有共同服务承担事务、字节、模型、诊断与数值。

## 长期记忆的研究启发

[LongMemEval（ICLR 2025）](https://arxiv.org/html/2410.10813v2)把信息提取、跨会话、时间、知识更新与适当保留未知纳入任务；其不同粒度实验指出，整会话会影响查找，进一步压缩成事实/摘要又会丢失细节。本项目据此选择保留完整 Journal，同时提供精确局部候选；这不是照搬其会话粒度，也没有据论文保证 Nous 的效果。

[Generative Agents](https://arxiv.org/abs/2304.03442)把观察与更高层 reflection 放在同一可检索记忆体系，并保留反思所据的材料关系。本项目保留“旧经历能形成新理解”的用途，但使用明确文稿版本及来源，不引入该模拟系统的调度、重要性阈值或人格运行器。

[A-Mem](https://arxiv.org/html/2502.12110v1)研究结构化笔记、关联生成及随新经验更新已有记忆属性。这里吸收可演化关联的方向；正式正文修改、概念身份与索引发布由程序 owner 采用。初期不为每次写入自动重写一批历史文稿。

## 内容与认知身份

[PROV-DM](https://www.w3.org/TR/prov-dm/)区分实体、活动、责任者与派生，并将 revision、quotation 等作为具有明确意义的派生关系。Nous 使用这些区分来避免把生成模型、认知作者、原件和文稿修订混成一个身份；不为此引入 RDF 存储或完整 PROV 服务。

[Web Annotation Data Model](https://www.w3.org/TR/annotation-model/#text-position-selector)指出位置选择会随源内容编辑失效，应指定所指表示。Nous 已有不可变 Artifact，因此使用 exact revision + TextView/parser profile + selector，旧位置固定旧正文。现有 TextView 不另行替换。

[SKOS Reference](https://www.w3.org/TR/skos-reference/)分别描述概念身份、标签、别名、说明、关系与有序集合。它支持我们把名称与概念身份分开；本轮自行决定 Concept 意义版本与标注固定策略。名称相同、向量相似时先发现候选，不强制合并。

## 事务与检索可见性

[SQLite Isolation](https://www.sqlite.org/isolation.html)说明 WAL 下读事务保持其一致快照，写入串行；因此在短 ReadTx 捕获 generation 和不可变引用，之后结束数据库事务再做模型和 Native 工作。

[Elasticsearch refresh 文档](https://www.elastic.co/docs/reference/elasticsearch/rest-apis/refresh-parameter)将写入与搜索可见性明确区分。本项目采纳这种外部合同区分，具体实现继续使用现有 SQLite + immutable WaveSnapshot，不增加 Elasticsearch、持续 refresh 线程或外部服务器。

## VCP 与 Journal 的对应

[VCP DailyNote](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/TVStxt/Dailynote.txt)强调主题明确的文稿、可回到正文的有序概念，以及基于多个事件的新反思。Nous 保留这种有序关系寻址，但正文可有多条局部路径；概念采用具有稳定意义版本，文章不必先标注才能使用。

项目《VCP 研读》区分长期资产与单次查询响应，明确 RiverMemo 结合曲线、图、双场与正文读出。本轮将实际认知版本转成这套已有输入，不把 Journal owner 搬进 Rust。普通质量分、最终排序键和独立支持继续保留各自意义。

## 比较后选定的结构

| 选择 | 本轮决定与原因 |
|---|---|
| 直接改写原 Markdown vs 不可变 revision | 保存 revision；纠正后旧引用有真实可读对象，现有 CAS 已能复用字节 |
| 每个术语独立服务 vs 共享文稿保存 | Note/Journal 共享有限 WrittenRecord；各自产品规则明确，避免重复版本机制 |
| 单个万能模型 vs 写作与标注分开 | 分开专一任务；模型无需理解身份、图权、采用状态 |
| 模型自动语义合并 vs 明确 Concept binding | 明确复用/新建；已知候选可由调用方一次授权，不逐项人工审批 |
| 每次保存强制全索引 vs 按需当前视图 | 保存快速完成，首次 current query 或显式 refresh 发布；无需通用后台平台 |
| 旧快照+增量叠层 vs 完整不可变资产 | 先完整快照并复用向量，确保旧图/几何不会影响当前认识 |
| 绑定所有旧引用失效 vs 显式 current_basis | 只传播明确依赖；历史引用和启发可以继续成立 |
| DBOS 立即接入 vs 实际持久后台消费者后接 | 本轮保持前台与可恢复代次差；真正需要自动续作时接入 DBOS |

## 尚待实际使用回答的问题

描述型 Concept 和局部路径是否显著改善用户问题的召回；概念候选清单多大适合所用模型；完整快照在真实资料规模下何时需要增量更新。这些由 journal-use 中的真实内容决定，不阻塞已选身份/事务/引用合同，也不通过预设指标替代工程判断。
