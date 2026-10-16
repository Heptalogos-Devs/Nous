# 社会认知

[返回 owner 目录](../INDEX.md)

Social 拥有有向 SocialRelation 和限定社会作用域的 LanguageConvention，保存稳定事项、不可变修订、独立证据、类型化程度和认识判断。Host 提供已经绑定的 opaque EntityRef；关系内容不授予 Host 或工具权限。

[领域合同](../../docs/specs/active/social/social.md)与[正式 wire](../../proto/nous/wave/v1alpha1/social.proto) 提供 adopt/revise、追加证据、get/list/history、生命周期、purge、有界选择、pair/neighborhood，以及分开的理解/表达惯例解析。官方 Client 使用 `social` namespace，CLI 使用 `social` 命令。管理分页与本次认知候选预算分别定义。

可信 `SourceReadScope` 指向同一 Subject 的来源，由 Material 解析实际来源根。owner 在读取正文前同时过滤来源许可、结构 scope 和有效时间；正文及追加证据在同一 read view 批量加载。`social_source_roots` 是可通过 owner 有界维护方法重建的派生索引，修订和追加证据同时更新它。缺失授权或无法解析来源时不传播相关认知。

带可信 usage 的 Query 冻结被授权的 exact refs，共享 lexical/dense 在生成候选时过滤，owner 最终核验。Projection 自动组织当前参与者关系与用语解释，保留候选和正文省略信息，表达另按实际受众使用。旧 lexical generation 缺少授权搜索字段时需要重建，不能退回未过滤搜索。

共享 `subject_reflection` 负责模型调用和先保存成果；[typed proposal/apply](../../apps/nous-core/src/model/social.ts) 只向正式 owner 提交。Social 不建立自己的模型 role 或执行链。真实 QQ 自动成长和自然后续行为仍需 Host 可信场景、共享反思与真实外部证据接线；手工 owner 烟测不证明自然学习。
