# Self

[返回合同目录](../../INDEX.md)

## Owner 与引用

`crates/self` 拥有 SelfFacet、NarrativeThread、修订、来源与生命周期。Runtime 继续拥有 Session/WorkContext/UseEvent，Core 拥有模型与公共宿主，Subject owner 拥有身份、能力供给和 Seed 来源。

SelfFacet 是可独立修订的事项：kind、完整 statement、scope、stance、exceptions、epistemic class、RevisionBasis、valid time 和 producer。kind 为 identity、role、capability_belief、limitation、tendency、value 或 preference；stance 为 adopted、tentative、aspirational 或 attributed。scope 保留自然语言说明，并可按现有 purpose、Entity、WorkContext、conversation 约束。

NarrativeThread 保存 theme、interpretation、exact cognition sources 与 basis。叙事重解释修订原 thread；它不重写历史来源。两类对象分别使用 `self_facet`/`narrative_thread` 和对应 `_revision` 引用，不与 Memory 共用对象族。

## 供给和来源

`SubjectCapabilities.self_cognition` 显式供给 Self，默认不启用。`subjects.provisionCapabilities` 只追加请求的能力，保存既有 Subject 和认知，不通过克隆更换身份。没有 Seed 不生成默认人格、传记或自我对象。

Seed 保存原文、版本和 adoption，接受原有声明格式和 `text/markdown`；Markdown/frontmatter 保持来源地位。Self 的 SeedBasis 指向已保存版本，以 `$` 或 `line:start-end` 定位真实源文档；locator 越界和 foreign Subject 来源被拒绝。Seed 不创建伪称真实接触的 Occurrence；设定和自我叙事使用明确的 narrative/simulated 等认识地位，不能冒充 observed 外部经历。

EvidenceRef 由 Material 核验实际 Occurrence 与 locator；cognition dependency 使用 exact revision。Self 的来源/叙事内容不授予工具或管理权限。

## 官方操作

Canonical `SelfService` 由官方 Client 的 `self` namespace 暴露：

- `adopt` 创建 facet 或 narrative 对象及初始 revision；
- `revise` 要求 exact target、当前 object epoch、内容和实际变化理由，同对象创建新 revision；无正文变化时保存操作结果而不制造新修订；
- `addEvidence` 为 exact revision 增加 support、counterexample 或 boundary_case，不因依据增加而全文重写；
- `get`、`getRevision`、`list`、`listRevisions` 提供当前、精确及有界历史视图；
- `setLifecycle` 执行 suppression/restore、withdraw/reaccept；
- `purge` 通过正式授权操作清除对象、修订和所属 Runtime 引用，保留必要幂等及 tombstone 语义。

操作使用现有 mutation envelope、owner lock 和 receipt。同 operation id、同请求重放既有结果；输入改变冲突。修订同时核验精确 head 与 epoch；历史读取保留真实旧内容。Self 修订与生命周期变化使依赖的当前解释进入 revalidation_required，不静默改写正文。

普通“记住/忘记”、称赞、纠正和不赞同是交谈或来源；显式 Self 操作才修改领域状态。主体自己的选择、遵守受托工作与采用价值分别解释。

## 当前施工范围

本功能分支已接通上述 owner、公开 RPC/Client 和数据库场景。模型形成/反思、自动 Self 选择与 Query/Serving、CLI 和正式 OpenClaw 的 Self 使用场景继续在同一功能内实现；完成状态由实际代码与运行证据确认。
