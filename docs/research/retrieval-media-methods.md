# Retrieval 与媒体实验方法

[返回研究入口](README.md)

研究结果以真实来源、Ground Truth 与可追溯 provenance 为基础。当前语料与 oracle 见 [研究语料](corpus/README.md)；当前观测见 [observations.md](observations.md)。

## 文本检索

Tracked 的 manifest 包含 6 个来源和 105 个语义单元；query oracle 包含 40 条查询。Relevant hit 必须通过 Memory provenance 追溯到 expected 或 acceptable source unit。Oracle 在查询前对照原始来源确认。

实验分为两条 track：

- **controlled**：先固定已形成的 Memory Authority，比较检索机制。
- **end-to-end**：通过真实来源、Observation、派生表示、Memory formation、embedding 和 query 完成完整流程。

同一 track 的 variants 使用同一 Authority state。最终查询通过 official Client 的 public recall API 执行。Formation coverage、条件检索召回和 end-to-end recall 分开统计。

当前测量的 variants 是 baseline 与 model-rerank；Topology/combined 需要真实 topology signal，本语料没有该 signal，因此未测量。指标包含 Recall@1/5/10、MRR、formation coverage、provenance precision、wrong-source/entity、stale-revision leakage、empty-result rate、p50/p95 latency 和可用时的 provider usage/cost。

## 媒体派生

Tracked 的媒体 manifest 包含 9 个样本。人工核对样本事实，再比较 description_only、direct_structured 和 describe_then_structure 的已提交表示、provenance、结构完整性与下游 recall。模型输出不能作为自身的 Ground Truth。

当前真实音频观测和质量缺口见 [observations.md](observations.md)。视频 frames 模式是已实现的显式 FFmpeg 输入路径；当前 Research 尚无 frames-mode 观测。

原始来源内容、媒体、响应、逐条 query 输出和运行日志位于 ignored 的 `data/research/`。
