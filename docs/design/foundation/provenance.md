# 精确来源与相关范围读取

## 两类用途

内容血缘描述一份 Artifact 使用了哪些输入、由哪次处理产生。认知支持描述某项关系或认识依据哪些材料、这些材料是共同前提还是独立支持。ResultStore 拥有前者；未来 Journal/Social 等领域及 Wave 明确提供的关系合同拥有后者。

血缘可返回终端根集合，但根集合本身不宣称每个根都独立支持某条结论。一段文字共同用 A、B 推出 C 时，它的根集合为 {A,B}；是否存在两份独立见证需要关系自己的 SupportGroup。

## 关系存储

将当前 inputs.group JSON 中需要遍历的关系规范化到同一份正式数据：

```text
result_inputs(result_id, group_ordinal, key, name, role)
input_parts(result_id, group_ordinal, part_ordinal,
            artifact_id, selector, representation, media_type)
part_origins(result_id, group_ordinal, part_ordinal, origin_ordinal,
             artifact_id, selector, transform_result_id?)
result_outputs(result_id, ordinal, name, artifact_id)
```

selector 继续复用 ContentSelection 的共同值。inputs 不再同时保存一份可独立更新的完整 group JSON。get/read_view 由这些关系恢复公开 InputGroup；各表通过 result、artifact 和有意义的序位建立索引/外键。

`adopt_in(tx)` 在同一个短事务写结果、parts、origins、outputs 和 CAS 正文映射。外部来源声明在其正式清单/领域中保存，不伪造成曾实际执行的模型处理。

## 公开读取

```text
provenance(refs) -> mapping[ArtifactRef, ProvenanceSummary]
ProvenanceSummary:
  terminal_roots                 # 全部来源血缘
  support_roots                  # source/reference/current_basis 路径的终端来源
  traversed_results/relations（按需，关系保留输入 role）
  complete
```

利用 SQLite WITH RECURSIVE 或按当前祖先 frontier 的批量查询，只遍历与请求 refs 有关的祖先。优先用规范化关系和一条递归 CTE；不读取全库 artifacts/inputs/outputs 后在 Python 遍历。

取根使用集合消重；解释路径只在调用者要求时返回可说明的实际路径，不能枚举 DAG 的所有组合路径造成指数膨胀。遍历以同一读事务的快照为准，迭代完成后释放，不跨模型调用持有数据库事务。

局部转换 origin 保留精确页/时段。一次结果包含多个输出时，通用血缘可以指向实际输入集合；某条认识的证据选择由它自己的精确引用限定，不能从“共同使用过”推出“共同支持”。

## 基础能力迁移

Wave、诊断、模型结果读取通过公开来源操作取数据，不查询 content 私表。Embedding 只存实际输入到向量行的关系，不再复制整个输入来源图。

Artifact/Result 的公开操作位于 `content/artifacts.py`、`content/results.py`，来源读取位于 `content/provenance.py`。模型、媒体与 Wave 消费这些共同能力。

## 使用结果

给定两个目标 Artifact，应只访问它们的祖先及相关身份；向库中添加无关历史不改变这次查询所读取的关系集合。同源多次派生保持同一根，多个真正根保留各自身份；未知根或环返回具体错误，不能默认认成新独立证据。

依据：SQLite [递归查询](https://www.sqlite.org/lang_with.html)、SQLAlchemy [CTE](https://docs.sqlalchemy.org/en/20/core/selectable.html#sqlalchemy.sql.expression.CTE)、W3C [PROV-DM](https://www.w3.org/TR/prov-dm/)。采用其现有能力，不引入 RDF 服务或通用知识推理引擎。
