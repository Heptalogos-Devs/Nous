# VCP 与 Native 冻结数值结果

[返回文档目录](../../../INDEX.md)

本目录保全 24 份既有 numerical golden 的原始 JSON bytes，包含输入、冻结 expected outputs、来源 commit/owner/probe 与原容差。2026-10-09 从测试 fixture 目录迁入，未改写其中的版本、数值、来源或评价。它们属于研究结果；仍需数值 oracle 的算法检查直接读取这一份数据，不在检查中重写 expected。

VCP 覆盖 EPA/training、anchors、graph/transport、Sense、Pyramid/gating、dual fields、fusion、projection、candidate/readout 及组合 pipeline，其 expected 来自独立冻结的 VCP source probe。`nous-wave-v1.json` 保留 Native 旧 baseline 输出，用于行为回归，不能据此声称独立数学证明。来源、方法与实际已验证范围见 [VCP conformance](../../vcp-conformance.md)。历史完整原生矩阵已由本轮Linux读取并保全在 ignored `data/research/results/linux-preservation-2026-10-09/numerical-originals/`；43份原JSON包含完整输入、expected、容差和来源，逐份匹配旧文件SHA-256，两端均有相同副本。
