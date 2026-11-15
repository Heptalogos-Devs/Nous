# Wave 参数与能力配置

## 共同配置接入

所有行为参数先有定义和实际消费者，再决定呈现层级。用户常用字段由 Python WaveConfig 维护；Native 数值字段由实际 Rust 类型导出。两者统一进入现行 Configuration 的 catalog/explain/update。

| 字段 | 初始值/语义 | 生效 |
|---|---|---|
| wave.native.threads | min(4, process_cpu_count() 或 1)，>=1 | restart |
| wave.default_program | dense | next_operation |
| wave.exploration_program | research | next_operation |
| wave.unavailable_program | text_only；默认程序缺少能力时采用 | next_operation |
| wave.programs | 按名称配置算子图 | next_operation |
| wave.query.top_k | Rust query.top_k 的默认与范围；本次调用可覆盖 | next_operation/call |
| wave.query.include_counterevidence | true | next_operation/call |
| wave.context.enabled | true；无上下文时不变换查询 | next_operation/call |
| wave.context.window | Rust context.window 的默认与范围 | next_operation/call |
| wave.context.mix | Rust context.context_mix 的默认与范围 | next_operation/call |
| wave.context.null_mass | Rust context.null_mass 的默认与范围 | next_operation/call |
| wave.context.role_weights | Rust context.role_weights 的默认与非负约束 | next_operation |
| wave.context.default_role_weight | Rust context.default_role_weight 的默认与范围 | next_operation |
| wave.context.segment_threshold | Rust context.segment_threshold 的默认与范围 | next_operation/call |
| wave.dedup.semantic_enabled | false | next_operation/call |
| wave.dedup.threshold | 0.92，-1..1 | next_operation/call |
| wave.fusion.rrf_k | Rust query.rrf_k 的默认与范围 | next_operation/call |
| wave.associations.relation_gains | related=1、supports=1、sequence=1；其它显式设置 | 新 build |
| wave.associations.enabled | true；无关系时与 VCP 输入相同 | 新 build |
| wave.native.build.* | 来自 AGS/事实/索引真实配置类型 | 新 build |
| wave.native.query.* | 来自 pipeline/readout/Thinking/DTSC 真实配置类型 | next_operation |
| wave.native.query.readout.*K / maxUnionCandidates | 每路实际召回数与唯一并集上限，沿用 Rust 导出名称 | next_operation |
| wave.native.query.controls.thinking_min_closure | Rust query.thinking_min_closure；Thinking 最低闭合度 | next_operation |
| wave.native.query.controls.candidates_per_group | 召回及候选联合的组内上限；Rust 默认2 | next_operation |
| wave.native.query.controls.ann_search_effort | 候选向量索引的搜索扩展；Rust 默认32 | next_operation |

候选预算、场预算、传播跳数、语言门控、双场 α、Ω、动态路线及读出参数保留为对应细分配置。普通查询使用当前程序、结果数量、情境与交付预算；研发入口可以指定程序。

召回及候选联合的 `candidates_per_group` 可由对应节点覆盖，独立于请求的 `passages_per_group`；更改交付片段数不会改变候选总体。`ann.search.search_effort` 可覆盖候选向量搜索扩展，该值只影响查询，不属于资产构建身份。USearch 对每次搜索使用配置扩展和本次请求数量中较大的值；分组收集的 `examined_rows` 是处理过的返回候选行数，不是索引内部访问的节点数。

## Rust 配置定义

serde 类型/Default 是数值字段的唯一执行定义。构建导出 JSON Schema、默认 profile 与 x-nous 元信息资源；Python 的配置扩展仅加载该静态资源，不启动 Native。jsonschema/现有配置目录承接形状与范围；Rust 输入边界执行实际校验。

Python 保留关系语义、上下文呈现与操作范围设置。参数归属变动直接迁移，不额外建运行时插件注册平台。缺少文档说明的上游字段，开发者从实际公式注明单位与作用，不根据名称猜测。

最低闭合度继续置于 wave.native.query.controls；候选数量由各 lane K 与 maxUnionCandidates 定义。组排名融合默认使用 `wave.fusion.rrf_k`，`rank.fuse` 可以声明自己的 k；局部定位独立使用 `passages.locate.rrf_k`。`top_k` 使用 `wave.query.top_k` 或本次请求值，上下文的 context_mix 映射为 `wave.context.mix`。这些数值默认和范围来自 Rust 静态资源，Python 不维护数值副本。

## 程序与失败

纯词项程序不调用 embedding。显式程序缺少其需要的向量或VCP资产时报告能力缺项；默认程序可按配置选择替代程序并记录实际执行名称。hybrid 使用原问题向量与词项的名次融合。

空种子、没有触发 wormhole、无独立路线是正常算法状态；不能当作强制回退错误。上游已经支持的诊断和完整性标记透传解释，不隐去未执行阶段。

## 新增实验参数

`wave.associations.interpretation_gain` 为有限非负数，缺省 0，新构建生效；打开后按 evidence-projection 使用解释结构。旧快照保留原实际值。

Native 的可选 pair_work_limit 缺省 null（完整计算），启用时是明确工作量上限，超过即在发布前返回具体错误；它不决定静默丢弃哪些出现项。由 Rust 类型导出单位、默认与约束。

多路径缺省采用 journal-projection 的成对 max 规则；原 VCP 单路径结果不经过额外混合。Thinking 的显式记录开关/额度由观察配置拥有，不在 binding 暗改。所有新字段接入现有 catalog/explain/update，不让示例 JSON 成为第二份配置权威。
