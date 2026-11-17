# Journal 与 Wave 的设计结论及依据

## 资料与决定的层次

本文依据项目的《VCP 研读》、Nous `56f2a18c7968fed1e5ae960530b37ebbf655f900`、其父提交 `fdb39e4`、上游 VCPToolBox `d5c019ebbac1f317c2938d655d2ce916e41b9164`，以及随后两轮设计讨论。

《VCP 研读》解释原算法与当时的 Nous 方案；其中“仅原始来源构图”是上一版决定，本轮按下面的关系/证据区分修订。原始讨论仍保留，正式专题写本轮结论。本文的多路径聚合与解释性支持组属于 Nous 新设计，未被称为上游原公式或已经获得实验改进。

## 已有数学资产的使用位置

| 原机制 | 源码/资料支持的用途 | Nous 的选择 |
|---|---|---|
| 有序 Tag | 保存叙述展开，候选曲线与河网/正文闭合共同读出 | Journal 概念出现与局部路径；多路径分开，不拼成伪长链 |
| AGS / EPA | 局部与全局几何、投影覆盖和集中度 | Wave 查询和概念整理的信号；不写成主体逻辑能力 |
| Residual Pyramid | 逐层提取未覆盖向量方向 | 辅助弱线索候选；保留实际层/解释能量 |
| Sense / 双场 | 已有关系上的有界传播、两种尺度响应 | 共用不可变资产与同一次观测 |
| RiverMemo | B/P/G/A 基线、拓扑奖励与条件排序修正 | quality 与 ranking 分开，外层不重复融合内部通道 |
| Thinking | 实际前驱状态支撑的候选连续组织 | 阅读路线；可供 Dream 消费，Dream 自由写作另有职责 |

上游 DailyNote 明确要求概念顺序随正文发展，感悟文稿可以从多个事件构造新的路径。由此，Nous 应保留派生 Journal 的新结构贡献，同时单独处理证据的独立性。

## Journal 的主要结论

正文是 Markdown，认知身份与正文修订由 Journal owner 持有；字节、精确位置与生成血缘复用 content。Concept 标注是绑定某个正文版本的独立派生结构，可重做，不要求每次标注都产生新的记忆版本。

Note 是主动记录的碎片；Episode 组织经历；Journal 组织对经历/知识的主观理解。不存在再复制正文的 MemorySemantics 层。后来的纠正采用原文修订，独立的后来回顾或不同观点单独成篇。详见 [Journal](../design/cognition/journal.md)。

使用 markdown-it-py 的 source map 而非按空行切割，是为了让代码块、列表和引用仍有自然语义。版本内的局部块键足够支撑当前精确选段，无需跨编辑永久段落身份系统。

## 关系贡献与独立证据

W3C PROV-DM 描述生成、使用与派生；它提供来源表达，并不替某条认知判断“有几份独立证据”。Provenance Semirings 的研究区分联合输入和可替代推导，为本轮区分 `{A,B}` 与 `{A}`/`{B}` 提供数学表达背景。本轮不采用多项式引擎，而使用明确的 SupportGroup。

选定的工程方法是：同一关系和规范化依据组的重复解释取最大贡献；真实 source occurrence 保留原 VCP 质量累积；解释贡献在固定出流校准前以显式增益加入；强路线 witness 继续要求实际独立来源。候选自己的全部根与一组共同前提相交时，该组不计完全独立支持。

VCP 现行 `independent_fraction_excluding` 的 0.15 是数值衰减底值。原始比例与该权重分别保存，避免把底值当成事实证据。原数学的中性/低支持行为与未知事实状态分别表达。

## 多路径与时间的可计算边界

多条局部路径按同一候选分别计算 P/G，选取一条实际路径的 P+G，B/A 只计算一次；原拓扑奖励上限保留。这样单路径退化为原定义，多个路径不产生额外候选票。该 max 选择仍可能受路径数量影响，必须观察冗余标注的结果，不能预设收益。

时间先决定合法版本/关系/概念视图，再构建几何。其后的可选门控保留 P≥0 和行和<1。采用列向量场时为 `(P^q)^T u`，收敛界由 1-范数得到。该推导支持数值有界性；时间函数如何反映具体认知意图仍由领域使用决定。本轮只落实明确来源子集实验，门控执行留在后续。

## 近期实验可以回答的未知

比较同一正文的无序、单路径、多局部路径、反向/打乱路径，观察变化来自哪条边和哪一项分数；比较原件与多次派生，观察新关系能否被使用而独立支持不膨胀；比较历史来源子集和当前视图，观察未来关系是否影响本不该影响的查询。

Concept 名称与完整描述的向量分布、不同领域的最佳门控、路径 max 的实际效果、时间偏好的用户收益仍需真实材料决定。代码支持对照，不把这些未知用统一默认常量包装成已证实结论。

## 主要来源

- [VCP DailyNote 写作与有序 Tag](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/TVStxt/Dailynote.txt)
- [TagMemo 深度文档](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/TagMemo_Wave_Algorithm_Deep_Dive.md)
- [RiverMemo Topology](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/RIVERMEMO_TOPOLOGY_V3.md)
- [当前排序与质量合同](https://github.com/lioensky/VCPToolBox/blob/d5c019ebbac1f317c2938d655d2ce916e41b9164/docs/RIVERMEMO_RRF_SCORE_CONTRACT.md)
- [Nous 当前 graph 与支持权重](https://github.com/Heptalogos-Devs/Nous/blob/56f2a18c7968fed1e5ae960530b37ebbf655f900/native/vcp/src/graph.rs)
- [W3C PROV-DM](https://www.w3.org/TR/prov-dm/)；[Green、Karvounarakis、Tannen：Provenance Semirings](https://doi.org/10.1145/1265530.1265535)
- [markdown-it-py 源位置](https://markdown-it-py.readthedocs.io/en/latest/using.html)
- [SQLite recursive CTE](https://www.sqlite.org/lang_with.html)；[SQLAlchemy CTE](https://docs.sqlalchemy.org/en/20/core/selectable.html#sqlalchemy.sql.expression.CTE)
- [NumPy mmap 读取](https://numpy.org/doc/stable/reference/generated/numpy.load.html)；[PyO3 detach/并行](https://pyo3.rs/main/parallelism)
- [Pillow 格式与编码](https://pillow.readthedocs.io/en/stable/handbook/image-file-formats.html)
- [structlog ConsoleRenderer](https://www.structlog.org/en/stable/console-output.html)；[OpenTelemetry Python](https://opentelemetry.io/docs/languages/python/instrumentation/)

本轮未执行这些实验，也没有给出实际性能或召回提升数值。修复依据是用户独立操作结果与源码责任关系；扩展依据如上明确区分。

## 请求去重的范围选择

本轮删除 Embedding 的实例级网络长锁，保留调用内精确去重、跨调用已采用缓存与事务唯一性。未采用前的两个并发请求允许重复执行。共享在途请求会涉及等待者取消、不同期限和根操作名额；当前没有必要为减少少量重复调用引入新的生命周期框架。这是对讨论中“按在途键合并”建议的工程收缩。
