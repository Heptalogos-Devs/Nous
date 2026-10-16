# SocialRelation 与 LanguageConvention

[返回合同目录](../../INDEX.md)

## Owner、身份与来源

`crates/social` 拥有主体的社会认识。Host 负责 EntityRef 绑定与账号、工具权限；Social 拒绝尚未在该 Subject 中绑定的实体。`social_cognition` 为独立可选能力，既有 Subject 默认关闭，通过正式 provision 显式追加。迁移 `0007_social_cognition.sql` 不替换既有 Subject、来源或认知。

事项使用稳定 object 与不可变 revision；所有写入复用 MutationEnvelope、operation id、精确 head/epoch、CognitiveClock、producer 和 workflow result 依赖。关系身份为 observer Subject、from、type、to、对称声明与结构 scope；惯例身份为 expression、meaning、referent 与结构 scope。scope.description 可修订，实体集合按无序集合规范化。跨事项变化不能冒充同一事项修订。

scope 的个人/双方 entities、group、channel、conversation、WorkContext、domain 同时约束。结构 scope 不授予来源传播权。可信 Host 在 `CognitionUsage.source_read` 提供同一 Subject 的 SourceReadScope；Material 解析实际 source roots，来源缺失、跨 Subject、不可用或闭包超预算时拒绝。来源许可和结构 scope 在检索与正文读取前执行。

## 类型与内容

| relation_type | degree | 结构 |
| --- | --- | --- |
| familiarity | ordinal: acquainted / growing / familiar / deep | 有向 |
| affinity | affinity: [-1,1] | 有向 |
| trust | ordinal: cautious / limited / stable | 有向，必须指定 domain |
| closeness | ordinal: ordinary / close / very_close | 有向 |
| collaboration | state: proposed / active / paused / ended | 可声明共同事项，由单对象投影两向 |
| mentor_of | 无 degree | 有向角色，逆向 view 为 mentee_of |
| member_of | 无 degree，必填 role | 人物/主体到群体，逆向 view 为 has_member |
| conflict_with | state: disagreement / ongoing / resolved | 有向或明确共同事项 |

不同类型不相加排名；缺少 degree 表示未定，不表示零。judgment 独立保存 hypothesis/supported/disputed、解释及可选估计与估计方法，未校准数字不称概率。statement、协调期待、有效时间、basis、追加支持/反证/边界案例、producer 和生命周期随完整 view 返回。

惯例保存 expression、显式 variants、meaning/referent、pragmatic_use 与上述认识和来源字段。表达及变体采用 Unicode NFC；只有显式 case_insensitive 才忽略大小写，不自动合并繁简、谐音或昵称。同词不同范围与不同含义可并存。supported 惯例需要带外部人物归属的实际交互 Evidence；主体生成或模拟的重复输出不能独立确认惯例，当前猜测可以保存为 hypothesis。

## 正式操作与消费

Canonical [SocialService](../../../../proto/nous/wave/v1alpha1/social.proto) 与官方 Client `social` 提供 adopt/revise、addEvidence、get/getRevision、list/listRevisions、select、pair/neighborhood、resolveConventions、setLifecycle 与 purge。CLI `social` 使用同一合同。

管理页上限 100，仅限制单页。select 是独立的当前认知选择，最多 2048 条、4 MiB 完整正文与依据；按 keyset 返回真实 next 和 omitted，不将缺席解释为不存在。有效时间与许可先于预算。pair 一次返回实际两向记录或明确未知；逆向角色作为带原 exact record 的 inverse view 表达，trust 不镜像。对称共同事项只写一个规范端点对象。

resolveConventions 保留原话，按指定 scope、实际时间点、预算与许可返回字面/显式变体候选、冲突和来源。understanding 与 expression 是独立消费；表达仍须按实际受众选择使用、解释或普通说法。语义补充使用同一 Query/Serving 管线。带可信 usage 的 Query 冻结 exact authorized refs，共享 lexical/dense 在候选生成时过滤，owner 再核验；没有可信 scope 的通用查询不传播 Social。Projection 按当前参与者、来源与受众组织完整社会认识，并报告候选及文本预算省略。

追加 evidence 不全文重写。跨 Self/Social 的 exact 依赖闭包负责 revalidation 与旧历史传播屏障；追加支持拒绝有界闭包中的跨 owner 循环。撤回、抑制和 purge 延续共享 Runtime/Serving/历史清理。来源根索引从 basis/evidence 派生，可用 owner 的有界维护方法重建，不成为第二份认识真值。

## 形成入口与验证范围

共享 subject_reflection 负责模型机会、真实来源、预算、成果保存与恢复。Core 的 [typed proposal/apply](../../../../apps/nous-core/src/model/social.ts) 接收 adopt/revise/add_evidence/keep，逐项返回实际结果；producer 必须为 SubjectReflectionText。此模块不创建 Social 模型 role 或独立执行链，不自动授予生命周期或 purge 权限。

`pnpm smoke:social` 使用隔离 PostgreSQL，验证 owner、来源/作用域拒绝、Query/Projection、独立方向、逆关系、单对象对称读取、追加证据、混合 owner 循环、历史屏障及超过管理页 100 的候选。它不声称真实 QQ 自然学习已通过；实际成长还需 Host 的可信事件与共享反思接线。
