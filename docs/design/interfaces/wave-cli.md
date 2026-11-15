# Wave 产品入口

## 命令

继续使用长期产品前缀 `nous tools`，不建立第二套实验服务。当前 `model/media/content/result/embedding` 命令继续有效。

```sh
nous --instance ./temp/nous-wave tools wave build --manifest ./collection.json --space chosen-text
nous --instance ./temp/nous-wave tools wave inspect "<完整wav标识符>"
nous --instance ./temp/nous-wave tools wave inspect "<完整wav标识符>" --concept "resource-ownership"
nous --instance ./temp/nous-wave tools wave query "<完整wav标识符>" "取消之后文件为何仍被占用" --program hybrid
nous --instance ./temp/nous-wave tools wave query "<完整wav标识符>" "此前相关经验" --context ./context.json --core "resource-ownership" --cue "协程退出与实际工作结束的区别" --explain
nous --instance ./temp/nous-wave tools wave query "<完整wav标识符>" "查找解决过程" --program research --thinking-stages ./stages.json
nous --instance ./temp/nous-wave tools wave query "<完整wav标识符>" "字面线索" --program fixed_set --vector-ref ./vector.json --candidate-set ./set.json
nous --instance ./temp/nous-wave tools wave read "<完整wav标识符>" "candidate-key" --sources
nous --instance ./temp/nous-wave tools wave segment "<完整wav标识符>" ./context.json
nous --instance ./temp/nous-wave tools wave cleanup --keep-recent 2 --retain "<需保留的快照>"
```

wav 标识符由共用可读标识符能力分配并持久保存，例子中的占位符替换为 build 实际返回的完整标识符，并保留引号。candidate-key 是集合内稳定 key，使用实际命中键并保留引号；hit 同时返回可直接用 content read 的精确 Artifact 与选区。

build 的 --space 指定向量空间；清单也指定了空间且两者冲突时会报错。清单本地路径相对其文件位置。query 的 --program 指定实例配置中的 Wave 程序；省略时使用实例默认程序，未具备所需快照能力时按配置的备用程序处理。重复 --core/--cue 顺序保持。--filter 接收 CandidateFilter JSON，--top-k 指定结果数量。

## 输入与默认呈现

query 普通文字即可使用。context JSON 提供有序片段，roles 和序位可读；Thinking stages 使用候选 key 数组和阶段名称，不要求内部矩阵/UUID。

普通返回显示排序、分数种类、原文片段及精确内容引用。--explain 在普通输出中展开命中依据；--json 返回完整结果，包括本次程序、通路、覆盖、情境和可用的诊断信息。

cleanup 显式删除不再保留的旧快照与无引用组件；当前快照、运行中的读取、正在准备的结果和正式反馈所需快照自动保留。`--keep-recent` 指定每个集合额外保留的最近快照数，`--retain` 可重复指定本次保留的快照。

--json 返回同一公开结果的完整类型，日志与进度到 stderr。read 在原 owner 取回真实原文，不让模型“补写”匹配段落。近重复折叠的其他来源可以展开。

## 通路与开发辅助

构建、查询、inspect、read 都是正式应用操作。CLI 不把清单转成另一份算法模型，也不重新调用模型或写索引。新入口以后直接复用 WaveOperations。

临时样本、一次性速度测量与故障定位脚本放 temp，不新增 nous dev/benchmark 平台。普通 --explain 的阶段耗时和规模已足够支持早期算法观察。

## 已有向量与固定候选

`--vector-ref` 读取 Embedding owner 的 VectorRef JSON：`space` 为完整 SpaceDefinition、`purpose` 为 `query`、`artifact` 为已采用向量矩阵的 ArtifactRef、`row_index` 为矩阵行。Wave 核对快照空间并通过 EmbeddingOperations.read_vector 读取；文字继续提供字面线索，原始查询向量复用该行。

`--candidate-set` 的 JSON 为 `{snapshot, rank_scope, entries:[{candidate, lanes:{lane:{score,rank}}}]}`。snapshot 使用 wav 标识符或 snapshot id，candidate 使用该快照的 key，rank 从一开始。`fixed_set` 程序读取候选的 `provided` 通路后执行局部定位；其他程序按其算子定义消费候选集合。空 entries 返回空命中；`--filter` 继续限制允许集合。

包含 `vcp.think` 的程序（如 `research`）可以使用 --thinking-stages，在本次观测上计算路线。通过 `wave.native.query.pipeline.spikeRouting.maxTransitionRecords` 设置记录额度；0 表示禁用。结果报告实际路线或未形成路线的状态。

清单和 context 文件的 selector 直接使用内容 SourceSelection。例如 `{"start_line":3,"end_line":7}` 读取原 Artifact 第 3–7 行，`{"start_byte":0,"end_byte":128}` 读取字节区间，`{}` 选择全文。候选坐标始终指向原件；文稿已经指定范围时，通过共同 restrict 保持候选位于该范围内。
