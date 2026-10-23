# VCP 与 Native 冻结数值结果

[来源与版本](../../../UPSTREAM.md)

本目录保全 24 份既有 numerical golden 的原始 JSON bytes，包含输入、冻结 expected outputs、来源 commit/owner/probe 与原容差。2026-10-09 从测试 fixture 目录迁入，未改写其中的版本、数值、来源或评价。它们属于研究结果；仍需数值 oracle 的算法检查直接读取这一份数据，不在检查中重写 expected。

VCP 覆盖 EPA/training、anchors、graph/transport、Sense、Pyramid/gating、dual fields、fusion、projection、candidate/readout 及组合 pipeline，其 expected 来自冻结的 VCP source probe。`nous-wave-v1.json` 保留 Native 旧 baseline 输出，用于行为回归。当前 crate 的 `baseline` 保留原 reference 实现，`baseline_golden` 直接读取本目录；原始文件路径、固定 commit 和 SHA-256 见 [源文件清单](../../../upstream-sources.json)。新 AGS 实现与这套 SVD EPA 历史基线分别保存。
