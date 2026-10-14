# QueryExpr 与 Rerank

[返回当前产品合同](../../INDEX.md)

## Owners

Runtime (crates/runtime) owns QueryExpr binding, QueryPlan, lane budgets and fusion. Retrieval (crates/retrieval) supplies exact candidates. Core owns query embedding, model rerank and External Resource host actions. Memory and Material validate/materialize their own Authority; Runtime validates its generic references.

## Binding 与预算

Atom 是 leaf candidate set；all 对 canonical identity 取交集，any 取并集。父级 hard constraints 由子节点继承，子约束只收窄当前子树；整棵表达式绑定到一个 Subject 与一个 QueryPlan。

表达式最多 64 nodes、depth 16。Lane、validation、topology 与 resource budgets 按 leaf 顺序预分配：每个 leaf 获得 `floor(total/leaves)`，前 `total % leaves` 个各增加一份。矛盾约束形成空集；validation 无分配时返回 Partial 和 `expression_budget_exhausted`。

`effort`、`limit`、`diagnostics`、`explore` 与 `materialize` 只能出现在 root。Typed entity/tag/schema/resource/external-object cues 与 exact `@ref` 分开。未知 modifier 拒绝。软偏好只排序通过 Authority 与 hard constraints 的候选，不产生候选。

## 执行与 Authority

执行顺序为 candidate lanes → 固定 RRF → bounded Authority validation/materialization → optional model rerank → final Authority revalidation → user limit。

Core 持有 query embedding、rerank 和 Resource host actions；Kernel 持有 BoundQuery、QueryPlan、candidate fusion 与 final validation。内部 validated_candidate_limit 最大为 64，不改变用户 limit。正式 query 需要非空 TextCue；NousQL 使用单一 Unicode 意图和可选 syntax islands，内部 typed QueryExpr 保留 composition。Resource ticket 由 Core finalize/release。

Model rerank 接收固定 candidate refs 与标准 `query/documents/top_n` 请求。Index 范围、唯一性与 finite score 必须通过校验；未返回候选保留 baseline tail。Required 调用失败使 operation 失败；optional 调用不可用时返回 baseline 与 degradation。回包后按原 BoundQuery 批量检查 revision/head/epoch/lifecycle/source/hard constraints；失效候选丢弃，operation 返回 `authority_changed_during_rerank`。Mutable exact target 在 bind 时固定到 revision 与 object epoch，执行期间变化时返回 `stale_exact_binding`，不得自动重绑。

Query work budget 与 Client response wait 来自 active `core_execution.opportunity`；Runtime 的执行许可在 preparation、activation 和 validation 间继承原 deadline，角色 timeout 不延长它。Serving 的同一 Arc activation view 随对应 execution ticket 保留，期限到期不以新 ticket 重新获得机会。

## Scores 与 lanes

Public HitScore 保留 baseline、preference、optional rerank、final scores 与 ranks。Rerank score 只属于当前 query；持久使用由 UseEvent 表示。QueryDiagnostics 是显式 opt-in，记录 lane availability、budget 与显式 topology work。

Normalized RRF baseline 位于 [0,1]。每个满足的 soft cue 增加 signed 0.02，总 preference clamp 到 [-0.06,0.06]。recent(axis) 只接受 occurred、observed、valid、formed、recorded；未知时间得 0；recency 为 `1/(1 + age/(30 days))`。Rerank ordinal affinity 为 `(k+1)/(k+rank)`。Candidate pool 上限为 64 个候选与 2 MiB text。

Lexical relevance 只来自 Lexical Serving hit。Query embedding 使用 preparation 固定的完整 Query Representation，包含 normalized temporal orientation 与当前 context；无 context TextCue 仍使用相同 representation preparation；按 exact text/space/producer digest 最多缓存 128 vectors。Required embedding 失败拒绝 operation；optional 的 lexical fallback 返回显式 degradation。

Cognitive profiles 是固定 registry：`baseline-rrf`、默认 `nous-node-potential-v1`、`vcp-dtsc-v9.2.1-adapter-v1` 和 `vcp-rivermemo-v3.1-adapter-v1`。Native 查询使用 bounded propagation 与 node-potential；VCP adapters 使用实际 EPA/residual 与冻结 reference 数值合同。独立 Native EPA/residual 算法保留研究参数与数值测试，不自动构建 Dense EPA 资产；当前公开 Query 的 Required residual sensing 返回 unavailable，不报告已执行。所有 profiles 共用同一 prepared representation、activation、Tag vectors 和 direct postings；profile selection 不改写 query intent/context。

Serving current/history 通过同一 family resolver 获取 record 与 opened artifact，直接组成 request snapshot；current publication 不成为 request view 的中转站。Historical identity 使用 owner-projected effective-state digest，历史 generation 不发布为 current。Request view 由 Arc 持有，Retrieval 通过弱 reader 注册保护实际使用的 generation；retired artifact 的 grace/research pin 保护与显式 reclamation 分离；query 不扫描整个 Serving 文件目录。

Host embedding 在 query RPC admission 校验 exact representation text、space、producer、dimension 和 finite vectors，然后作为同一 typed semantic embedding 显式保存在 operation activation。Concept matching、dense 与 cognitive lanes 消费该 material，不通过 task-local channel 找回输入。受控本地 provider 仍可作为测试/研究实现，并在单个 request 内共享一次 embedding 调用。

Preparation 的 reference binding table 复用同一 requested reference 在 exact/context/structural roles 的绑定，但保留 role origins 和 mutable epoch。Final Authority validation 是新鲜检查，不复用 preparation cache。Generic mutable exact refs 变化时丢弃候选；drop diagnostic 不替代 rejection。
