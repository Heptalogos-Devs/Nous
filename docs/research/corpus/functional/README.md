# 手工认知功能语料

[返回语料入口](../README.md)

[scenarios.json](scenarios.json)保留三个手工场景：[Tide deployment rule](scenarios.json)、Aya journal preference、snapshot/build-cache lease analogy。[queries.json](queries.json)包含 15 个 prepared intents；[manifest](manifest.json)维护语料身份。

原研究通过产品 Client 提交 Subject、Session、Observation 和 WorkContext，显式授予有界 maintenance，再检查 Tag create/reuse、alias merge、distinct concepts、one-off joke、Association support、recurrence 和 ambiguity。下述是保留的历史方法和观测。

当时 PrepareQuery 固定 semantic representation 与 exact context；baseline/native/DTSC/RiverMemo 使用相同 semantic input。自动验证使用 fake/deterministic provider，不调用真实 embedding、rerank 或模型；原始 text-only compatibility 独立报告。runner 连接已经运行的 public Core，结果写入 ignored `data/research/`。

2026-10-06 的 deterministic public smoke 已通过三场景形成质量检查，运行 15 intents × 4 profiles，并验证同一 intent 的 prepared semantic text 不因 profile 改变。六个 selected raw-text cases 在四 profiles 下 的 first relevant source 均为 rank 1。Fake embedding 为线路验证向量；query metrics 原样保留，部分查询仍有 temporal unknown、truncation 或排名不足，不把线路执行解释为语义质量通过，也不宣布 profile winner。当前 source/case 级结果保存在 ignored `data/research/cognitive-functional/`。

2026-10-09 全仓重整中，旧 synthetic runner、专属 deterministic provider、可选 longitudinal 分支及命令入口已退役。三个场景、22个事件、15个 intent、四 profile 名称、source checksum、oracle 和上述旧结论保持原值；本地保全摘要位于 `data/research/results/deep-rebase-preservation/functional/`。当前 Windows checkout 未发现旧 raw results 目录，未据此删除其他磁盘或旧材料。现行认知能力通过 compact owner/public 检查和真实 CLI/MCP／Portable 任务验证，见[重整观察](../../deep-rebase-2026-10-09.md)。
