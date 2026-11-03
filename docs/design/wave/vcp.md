# VCP 数值闭包与可用能力

## 采用版本

采用 VCPToolBox `d5c019ebbac1f317c2938d655d2ce916e41b9164` 的实际计算与当前排名合同。旧 Nous `legacy/native/vcp` 从 `d1b40c8` 适配，是解耦与来源扩展的复用材料；不能仅搬旧代码就宣称已完整集成当前 VCP。

本节的版本固定用于实施输入，不要求每次联网追新。后续明确升级时改此处和实际源码来源说明。

## 计算闭包

| 计算 | 输入与实际产物 | 主要源码 |
|---|---|---|
| AGS 局部 Explain / 全局 Span | 原始 Concept 向量、共现邻居；残差比、anchor gain、主题基、均值、覆盖 | memo_geometry.rs |
| 事实与传输资产 | 有序来源出现、独立贡献；正权 CSR、枢纽修正、来源质量、行预算 | memo_artifact_builder.rs |
| EPA | 中心化查询、主题基；解释率、域内门控、逻辑深度等指标 | memo_pipeline.rs |
| Residual Pyramid | 查询残差、Tag ANN、图内候选；接受层、贡献、剩余能量与路由尾残差 | memo_pipeline.rs |
| 门控与种子 | explicit/core、literal、implicit、语言置信门控；有类型种子 | memo_pipeline.rs |
| Sense | 前驱状态、动量、有限跳、FIR、返流抑制；真实节点/边电流及可选 transitions | memo_sensing.rs |
| 动态 Wormhole | 更深层残差、已有正边；路由选边及实际电流 | memo_sensing.rs |
| Fusion / 双场 | 原查询、种子/观测、同图两尺度场；enhanced/local/transfer 向量 | memo_pipeline.rs |
| 连通流域投影 | 核心与真实前驱、质量/数量预算；各场投影支持集 | memo_projection.rs |
| RiverMemo | 多路候选、有序曲线、正文闭合、拓扑/形态/Ω、直接锚；质量与排序键 | rivermemo_topology_v3.rs、rivermemo_wormhole.rs |
| Thinking | 同次完整 transitions、阶段候选集合；前驱支撑的选取路线 | memo_thinking.rs |
| 近重复折叠 | 排名候选与向量；保序代表、被折叠引用 | ResultDeduplicator.js 的数值部分 |

同时保留原 query、enhanced、local、transfer、BM25、direct-anchor 的候选通道与配置，不把全部召回缩成 query ANN 后重排。各通道采样上限和最终 top_k 分开。

## 正式中间结果

一次 QueryObservation 持有相同 snapshot 的查询信号、Pyramid、Sense、双场、原始 Concept 向量池与候选所需诊断。search 内只运行一次核心 pipeline，RiverMemo 与 Thinking 共用它。观测是 Rust-owned Arc 对象，通过 Python 生命周期显式释放，不建立 TTL 字符串句柄服务。

Thinking 需要完整有效前驱 transitions。记录缺失或被裁剪时返回明确不可用；允许其余查询结果仍可交付，但不得伪造路线。路径检查使用 hop、previous_id 与真实 source/target 关系，排除立即回流。

## 保留的数值语义

保留上游局部残差范数比与解释能量比的区别；几何局部/全局与查询双场局部/迁移的区别；标量场支持与投影支持的区别。只接受图内残差标签，按既定 Top-K 内过滤，不能改变成无限补位。

动态 wormhole 只沿真实正 CSR 出边选择，不能根据相似度新增事实边。seed 层、尾残差、边流事件和保留的 transitions 各有计数。源 field 与传播实测电流不能互换。

传输矩阵使用非负值并满足实际行预算；有界传播、hub 抑制和质量/状态预算全部导出。不得为接入负关系直接向原算法塞负权。

## 来源扩展

保留旧 Nous 对 `independent_fraction(file_id)` 的多根扩展：改为 candidate 的整个 root set 排除。当前上游数值 `max(0.15)` 下限是算法的有界倍率，仍保留在对应 profile；另提供未加下限的独立来源比例用于解释，不能把下限说成事实置信度。

最新 wormhole 路线资格及 Thinking 使用同一扩展，防止总结由 A/B 派生后又把 B 算独立支持。来源缺失显式诊断；本轮构建有 root mapping 时不走“未知就完全独立”的隐含路径。

## 边界剥离

从上游剥离 rusqlite、N-API、全局 registry、计时清理器、日记路径与 embedding API；它们由 Python owner 和冻结输入替代。数值顺序、阈值、独立资格、输出版本及诊断保留。改编涉及本地 input provider 的实际依赖闭包，不只复制入口函数。

有关计算接口与文件映射见 [采用依据](../../research/vcp-adoption.md)。许可证随实际移植源码保留，不能将 VCP 源码标成项目自创 Apache 实现。
