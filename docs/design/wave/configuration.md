# Wave 参数与能力配置

## 共同配置接入

所有行为参数先有定义和实际消费者，再决定呈现层级。用户常用字段由 Python WaveConfig 维护；Native 数值字段由实际 Rust 类型导出。两者统一进入现行 Configuration 的 catalog/explain/update。

| 字段 | 初始值/语义 | 生效 |
|---|---|---|
| wave.native.threads | min(4, process_cpu_count() 或 1)，>=1 | restart |
| wave.query.default_mode | hybrid | next_operation |
| wave.query.top_k | Rust query.top_k 的默认与范围；本次调用可覆盖 | next_operation/call |
| wave.query.include_counterevidence | true | next_operation/call |
| wave.context.enabled | true；无上下文时不变换查询 | next_operation/call |
| wave.context.window | Rust context.window 的默认与范围 | next_operation/call |
| wave.context.decay | Rust context.decay 的默认与范围 | next_operation/call |
| wave.context.mix | Rust context.context_mix 的默认与范围 | next_operation/call |
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
| wave.native.query.controls.candidate_budget | Rust query.candidate_budget；控制实际候选预算 | next_operation |
| wave.native.query.controls.thinking_min_closure | Rust query.thinking_min_closure；Thinking 最低闭合度 | next_operation |

候选预算、场预算、传播跳数、语言门控、双场 α、Ω、动态路线/读出参数全部保留为对应细分配置。不要用一个 effort 值覆盖或丢掉它们。普通用户以 mode、top_k、context、解释选项使用；高级目录可以查看参数含义和实际值。

## Rust 配置定义

serde 类型/Default 是数值字段的唯一执行定义。构建导出 JSON Schema、默认 profile 与 x-nous 元信息资源；Python 的配置扩展仅加载该静态资源，不启动 Native。jsonschema/现有配置目录承接形状与范围；Rust 输入边界执行实际校验。

Python 保留关系语义、上下文呈现与操作范围设置。参数归属变动直接迁移，不额外建运行时插件注册平台。缺少文档说明的上游字段，开发者从实际公式注明单位与作用，不根据名称猜测。

query 控制资源中的候选预算与最低闭合度置于 wave.native.query.controls，调用时展开为 Native Options 的同名字段。top_k、rrf_k 分别保持 wave.query.top_k、wave.fusion.rrf_k 唯一配置路径及原有覆盖优先级；上下文的 context_mix 映射为 wave.context.mix。它们的默认和范围均来自 Rust 静态资源，Python 不维护数值副本。

## 模式与失败

明确请求 lexical 不调用 embedding。明确请求 dense/tagmemo/rivermemo/dtsc 但资产缺失时报告能力缺项。hybrid 按排名专题选择完整 RiverMemo 或缺图时的 lexical/dense 融合，结果必须列出实际 used/skipped 及原因；当前开发完成条件仍要求用具备完整资产的集合实际运行完整 VCP。

空种子、没有触发 wormhole、无独立路线是正常算法状态；不能当作强制回退错误。上游已经支持的诊断和完整性标记透传解释，不隐去未执行阶段。
