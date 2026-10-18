# Self 领域

[返回 Rust owner 目录](../INDEX.md)

Self 维护主体采用的自我认识和组织性叙事。SelfFacet 与 NarrativeThread 使用各自稳定引用及精确修订；它们共用领域内部的事务、历史与生命周期机械，不写成 tagged Memory。

`SelfService` 通过已有 MutationEnvelope 提交，复用 Material 来源核验、Subject capability、认知时钟、producer 和 model-results。statement、scope、stance、exceptions、valid time 与来源随修订保存。增加支持或反证可独立于正文修订。

公开合同见 [Self Spec](../../docs/specs/active/self/self.md)，wire 定义见 [self.proto](../../proto/nous/wave/v1alpha1/self.proto)。官方 Client 的 `self` namespace 与 Core/Kernel 使用相同操作。

管理 list/history 保留真实分页、不可用状态与完整 evidence。`read_many_in` 在调用方 read view 中分别批量加载正文和追加依据；候选读取不以管理页大小作为领域总量限制。自动上下文先筛可用性、时间和结构 scope，必需身份独立取得；反思围绕 source/target/一跳依赖与共享检索 refs 读取，条目和字节预算省略整项并报告。

域 owner 的 `Record::context_text()` 统一生成可读正文，读取 wire 的 `contextText` 与运行时 ContextSource 共用它。当前修订、历史前缀、结构 scope、来源依据及时间均来自原记录；不另存文档状态。正文更新进入既有 Projection 内容 digest，ContextCompiler 按实际变化 RESET 缓存。
