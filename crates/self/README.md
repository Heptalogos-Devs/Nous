# Self 领域

[返回 Rust owner 目录](../INDEX.md)

Self 维护主体采用的自我认识和组织性叙事。SelfFacet 与 NarrativeThread 使用各自稳定引用及精确修订；它们共用领域内部的事务、历史与生命周期机械，不写成 tagged Memory。

`SelfService` 通过已有 MutationEnvelope 提交，复用 Material 来源核验、Subject capability、认知时钟、producer 和 model-results。statement、scope、stance、exceptions、valid time 与来源随修订保存。增加支持或反证可独立于正文修订。

公开合同见 [Self Spec](../../docs/specs/active/self/self.md)，wire 定义见 [self.proto](../../proto/nous/wave/v1alpha1/self.proto)。官方 Client 的 `self` namespace 与 Core/Kernel 使用相同操作。
