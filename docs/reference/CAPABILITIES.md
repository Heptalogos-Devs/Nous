# Capability Composition 当前参考

[返回文档目录](../INDEX.md)

当前 process/Subject capability set 为 Memory 与 Self（self_cognition）；Self 的 Subject 默认值为 false。Subject 创建时保存展开后的 typed capability set；默认配置变化不改写既有 Subject。

Memory-only Subject 提供 Memory formation/query/Serving、UseEvent、restart/rebuild、suppression/restore 和 purge。

当前 wire contract 在 `proto/nous/wave/v1alpha1/types.proto` 的 `SubjectCapabilities` 中声明 `memory` 与 `self_cognition`；Rust/TypeScript bindings 由 Buf 生成。Self/Social/Motivation 的长期语义见 Architecture-Vault。

Query CapabilityPolicy 独立声明 textEmbedding、multimodalInterpretation、residualSensing、rerank、queryConceptEnrichment 的 forbidden/optional/required。Concept enrichment 默认 off；model mode 仍须遵守 queryConceptEnrichment，forbidden 不调用模型，required unavailable 明确失败/partial，optional 才允许 degradation。TagDirect/plain text 不是概念模型的附属功能。

Memory MicroSystem 关闭时 Material/Evidence 的历史查询仍可运行。As-of 不恢复已撤销的当前 Memory 权限，也不绕过 purge/物理缺失。Query model activity、explicit formation、ReportUse 和 Maintenance grant 是分开的授权路径；反馈不等于长期 Authority mutation 授权。

`subjects.provisionCapabilities` 显式追加请求的能力，不重建既有 Subject 或清空认知。未供给的领域不生成默认对象；Self-only 查询与投影不依赖 Memory 供给。
