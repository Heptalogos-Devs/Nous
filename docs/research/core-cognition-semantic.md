# Core Cognition Semantic Qualification v2 入口合同

[返回 Research](README.md)

## 执行边界

Cognition / Agent Runtime Rebase 已完成 Squash Merge，v2 在一个独立研发分支和同一 PR 中修正预算/CLI/runner 后，使用 fresh Subjects 执行。长期语义依据 [Vault `5b96c63`](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/5b96c63da34b0a4c697b6961ae10ba6aa4de3ee1/docs/Nous-Wave/TARGET_DESIGN.md)。

[2026-10-07 v1 报告](core-cognition-2026-10-07.md)和[稳定结果 metadata](corpus/core-cognition/results-2026-10-07.json)是不可变的已执行证据。v2 不复用 v1 Subject Authority，也不改写 manifests/oracles 来隐藏其观测。[v2 实际报告](core-cognition-2026-10-08.md)与[稳定结果](corpus/core-cognition/results-2026-10-08.json)记录本轮 calibration、轨迹与 sealed。一次性 qualification runner 和专属脚手架测试已在 2026-10-09 退役；本页保留当时的方法与评价边界，持续认知工作使用当前 [Agent 入口](../agent/README.md)。

## Cognition trajectory

先复用 [Simon](corpus/core-cognition/simon.json) 与 [CPython](corpus/core-cognition/cpython.json) 的真实来源，按实际来源发展顺序执行 initial formation → later sources → concept maintenance → 自然适用的 consolidation → current → history → as-of。

Formation basis 表示实际输入，provenance 保留真实 lineage；epistemic judgment 是可修订的解释。错误日期、归因、Tag overmerge、弱 Schema 或持续误解属于认知质量观测，不作为结构损坏自动停止。PEP 703 的初次错误应继续观察后续 correction、persistence、propagation、current recall 与 historical fidelity。

硬阻塞是 cross-Subject、无效/缺失 basis、错误 immutable identity、mutable-head substitution、越权、幂等/receipt 损坏、purge bypass、future Authority/Serving leakage 或 query-local hypothesis 绕过 owner。保存原始失败和实际 execution identity；后来的成功不覆盖较早观测。

## 查询与 context calibration

比较 raw pronoun/deictic intent、foreground WorkContext text、pinned Entity、pinned Tag、exact cognition/Session Runtime 与 Host 显式 contextualized intent。选择少量能够区分设计问题的 ablation，不跑全组合。普通自然语言均可执行；显式 referent 是质量增强。

准备阶段冻结 QueryContextSnapshot、representation、TemporalFrame 与 projection，保持 provider-free；retrieval profiles 消费同一 activation。Plain baseline 通过显式 capabilities/config 禁止 enrichment/rerank，不存在产品 text-only compatibility 模式。Source/oracle 仅用于评价或明确实验 arm，不进入自动 formation 或 plain intent。

## Role × Execution Profile

围绕 memory_formation、concept_maintenance、memory_consolidation 选择少量可能改变部署决定的 ExecutionProfiles。记录每 role 的 semantic quality、latency、provider/token usage、实际成功 producer、attempt 和 fallback；不以模型营销档位预先决定角色。

初次形成错误与后续纠正分别评分；source recall 不能证明 claim 正确。多 regions、representations 或不同模型共享一个 source root 时仍是同源，不能增加独立佐证。

## Retrieval 与 sealed

形成可用 calibration Authority 后才比较 plain baseline、context、off/existing/model concept enrichment、direct Tag、associative exploration、current/history 和 selected profiles；所有 arm 禁止 rerank。多跳增量必须提供实际 activated route 与超出 direct attachment 的结果，保留 truncation/degradation。

Simon + CPython v2 回答 Role × Execution、context 价值/干扰、enrichment 默认值、Native/VCP 增量、错误修正/持续和历史忠实性后，才决定是否执行 [Rust](corpus/core-cognition/rust.json) 与 [Kafka](corpus/core-cognition/kafka.json) sealed。Kubernetes Sidecar 是 reserve。Fresh version 固定 source/oracle/model/Prompt/schema/config/embedding/code/Vault identities；sealed 不在运行中改变这些身份。

## Evidence 与恢复

v2 review 的 `reviewed_for_retrieval` 表示已阅读实际认知与 exact source basis、结构正常且可用于当前研究；`structural_blockers` 与 `semantic_observations` 分开记录。错误日期、错误归因和概念误合并不阻塞后续检索。Sealed lock 的 calibration `PASS` 表示经过上述复核并形成了可用部署判断，不表示认知全部正确。检索比较冻结 cut，后续反馈或修订另存轨迹与新 cut。

运行只经普通 Core 和 official Client；第三方原文、model trace 与逐次回执留在 ignored data，tracked 报告记录自写判断、短 locator、refs、hash 和实际指标。operation intent 在 RPC 前保存；已有 proposal 的基础设施重试复用同一 proposal 和 operation identity。

按轨迹记录初次错误、后来纠正、持续与传播、current recall、history/as-of fidelity，以及 context/Role × Execution 的收益和干扰。保留 source Recall@1/5/10、MRR、错误来源、stale leakage、empty rate、latency/usage；absence oracle 的分数保持 null。状态仅用 PASS、FAIL、NOT_RUN、BLOCKED。
