# Wave 产品入口

## 命令

继续使用长期产品前缀 `nous tools`，不建立第二套实验服务。当前 `model/media/content/result/embedding` 命令继续有效。

```sh
nous --instance ./temp/nous-wave tools wave build --manifest ./collection.json --space chosen-text
nous --instance ./temp/nous-wave tools wave inspect "<完整wav地址>"
nous --instance ./temp/nous-wave tools wave inspect "<完整wav地址>" --concept "resource-ownership"
nous --instance ./temp/nous-wave tools wave query "<完整wav地址>" "取消之后文件为何仍被占用" --mode hybrid
nous --instance ./temp/nous-wave tools wave query "<完整wav地址>" "此前相关经验" --context ./context.json --core "resource-ownership" --cue "协程退出与实际工作结束的区别" --explain
nous --instance ./temp/nous-wave tools wave query "<完整wav地址>" "查找解决过程" --mode rivermemo --thinking-stages ./stages.json
nous --instance ./temp/nous-wave tools wave query "<完整wav地址>" "字面线索" --mode rivermemo --vector-ref ./vector.json --candidate-pool ./pool.json
nous --instance ./temp/nous-wave tools wave read "<完整wav地址>" "candidate-key" --sources
nous --instance ./temp/nous-wave tools wave segment "<完整wav地址>" ./context.json
```

wav 地址由共用可读地址能力分配并持久保存，例子中的占位符替换为 build 实际返回的完整地址，并保留引号。candidate-key 是集合内稳定 key，使用实际命中键并保留引号；hit 同时返回可直接用 content read 的精确 Artifact 与选区。

build 的 --space 覆盖清单未指定值；两处都给且冲突时明确报错，避免静默选择另一向量空间。清单本地路径相对其文件位置，与 shell cwd 无关。重复 --core/--cue 顺序保持。query 的 --filter 接收 CandidateFilter JSON，--top-k 指定结果数量。

## 输入与默认呈现

query 普通文字即可使用。context JSON 提供有序片段，roles 和序位可读；Thinking stages 使用候选 key 数组和阶段名称，不要求内部矩阵/UUID。

普通返回按 Native 数组顺序显示排名、ranking_score、可用的 quality_score、score_kind、正文片段、概念匹配、精确短引用和来源摘要。--explain 展开本次 lane、quality/ranking score、实际算法版本、context/cue 作用、传播/反证路线与裁剪信息。默认不打印向量、全部 CSR 和全部参数。

--json 返回同一公开结果的完整类型，日志与进度到 stderr。read 在原 owner 取回真实原文，不让模型“补写”匹配段落。近重复折叠的其他来源可以展开。

## 通路与开发辅助

构建、查询、inspect、read 都是正式应用操作。CLI 不把清单转成另一份算法模型，也不重新调用模型或写索引。新入口以后直接复用 WaveOperations。

临时样本、一次性速度测量与故障定位脚本放 temp，不新增 nous dev/benchmark 平台。普通 --explain 的阶段耗时和规模已足够支持早期算法观察。

## 已有向量与固定候选

`--vector-ref` 读取 Embedding owner 的 VectorRef JSON：`space` 为完整 SpaceDefinition、`purpose` 为 `query`、`artifact` 为已采用向量矩阵的 ArtifactRef、`row_index` 为矩阵行。Wave 核对快照空间并通过 EmbeddingOperations.read_vector 读取；文字继续提供字面线索，原始查询向量复用该行。

`--candidate-pool` 的 JSON 为 `{snapshot, entries:[{candidate, lanes:{lane:{score,rank}}}]}`。snapshot 使用明确 wav 地址或 snapshot id，candidate 使用该快照的 key，rank 从一开始。固定池适用于 rivermemo、hybrid 和 dtsc；系统执行一次 observe 后直接读出给定候选，不再次 ANN/BM25。空 entries 返回空命中；`--filter` 继续限制允许集合。

Thinking 在同一次观测上执行。通过 `wave.native.query.pipeline.spikeRouting.maxTransitionRecords` 明确设置记录额度；0 表示禁用。结果分别报告 records_disabled、records_missing、records_truncated 或 no_grounded_route，非法请求和计算失败保留错误。查询不改变配置中的记录额度。

清单和 context 文件的 selector 直接使用内容 SourceSelection。例如 `{"start_line":3,"end_line":7}` 读取原 Artifact 第 3–7 行，`{"start_byte":0,"end_byte":128}` 读取字节区间，`{}` 选择全文。候选坐标始终指向原件；文稿已经指定范围时，通过共同 restrict 保持候选位于该范围内。
