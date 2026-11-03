# 能力与源码研究

[项目宪纲](https://github.com/Heptalogos-Devs/Architecture-Vault/blob/main/项目宪纲.md)定义产品目标；[重构进展](../progress-report.md)记录 Python/Native 当前能力与缺口。后续在当前领域 owner 中完善功能，依据实际需求选择成熟依赖和必要计算。旧应用实现可按需从 Git 历史研究。

A-Memorix 的检索、画像、Episode、导入、图谱、维护、纠错、Bundle、图片和管理操作构成完整功能基线。参照 `research/a-memorix-contracts.json` 与真实 MaiBot 消费者，公共参数和结果由边界适配器转换为 Nous 的领域操作。认知、数据库和其他原生消费者使用 Nous 自己的模型。

VCP 与 NousQL 的现有 Native 计算及其来源见 [native README](../../../native/README.md) 和 [VCP 来源](../../../native/vcp/UPSTREAM.md)。真实用户数据与公开合同按[数据边界](migration.md)处理。
