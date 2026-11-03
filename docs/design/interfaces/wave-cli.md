# Wave 产品入口

## 命令

继续使用长期产品前缀 `nous tools`，不建立第二套实验服务。当前 `model/media/content/result/embedding` 命令继续有效。

```sh
nous --instance ./temp/nous-wave tools wave build --manifest ./collection.json --space chosen-text
nous --instance ./temp/nous-wave tools wave inspect wav:<实际地址>
nous --instance ./temp/nous-wave tools wave inspect wav:<实际地址> --concept resource-ownership
nous --instance ./temp/nous-wave tools wave query wav:<实际地址> "取消之后文件为何仍被占用" --mode hybrid
nous --instance ./temp/nous-wave tools wave query wav:<实际地址> "此前相关经验" --context ./context.json --core resource-ownership --cue "协程退出与实际工作结束的区别" --explain
nous --instance ./temp/nous-wave tools wave query wav:<实际地址> "查找解决过程" --mode rivermemo --thinking-stages ./stages.json
nous --instance ./temp/nous-wave tools wave read wav:<实际地址> candidate-key --sources
nous --instance ./temp/nous-wave tools wave segment wav:<实际地址> ./context.json
```

wav 地址由共用可读地址能力分配并持久保存，例子中的占位符必须替换为 build 实际返回。candidate-key 是集合内稳定 key；hit 同时返回可直接用 content read 的精确 Artifact 与选区。

build 的 --space 覆盖清单未指定值；两处都给且冲突时明确报错，避免静默选择另一向量空间。清单本地路径相对其文件位置，与 shell cwd 无关。重复 --core/--cue 顺序保持。query 的 --filter 接收 CandidateFilter JSON，--top-k 指定结果数量。

## 输入与默认呈现

query 普通文字即可使用。context JSON 提供有序片段，roles 和序位可读；Thinking stages 使用候选 key 数组和阶段名称，不要求内部矩阵/UUID。

普通返回按 Native 数组顺序显示排名、ranking_score、可用的 quality_score、score_kind、正文片段、概念匹配、精确短引用和来源摘要。--explain 展开本次 lane、quality/ranking score、实际算法版本、context/cue 作用、传播/反证路线与裁剪信息。默认不打印向量、全部 CSR 和全部参数。

--json 返回同一公开结果的完整类型，日志与进度到 stderr。read 在原 owner 取回真实原文，不让模型“补写”匹配段落。近重复折叠的其他来源可以展开。

## 通路与开发辅助

构建、查询、inspect、read 都是正式应用操作。CLI 不把清单转成另一份算法模型，也不重新调用模型或写索引。新入口以后直接复用 WaveOperations。

临时样本、一次性速度测量与故障定位脚本放 temp，不新增 nous dev/benchmark 平台。普通 --explain 的阶段耗时和规模已足够支持早期算法观察。
