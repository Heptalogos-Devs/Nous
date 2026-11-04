# 正文标注提案与正式绑定

## 层次

模型提案保持局部 concepts / occurrences / paths，小型输出字段可以增加 `reuse_key` 来引用本次给定候选。正式 AnnotationSet 由程序补 body revision、parser profile、producer 和 Concept 意义绑定；存储共享现有 AnnotationProposal 的中立值定义。

每个 occurrence 指向精确正文块/选区。memberships 由该候选范围实际覆盖的 occurrence 生成；paths 保存显式顺序，允许重复概念和多条局部路径。没有自然路径可以为空，不要求为了 VCP 伪造边。

## 操作

```text
propose_annotation(record_revision, known_concepts?, task?) -> proposal_result
adopt_annotation(record_revision, proposal_result, bindings?,
                 expected_active_annotation, activate=true) -> AnnotationRef
read_annotation(annotation_ref) -> exact body + bindings + paths
```

提案可由人提供，同样走校验与采用。默认 annotate 命令可 propose 后 adopt，`--draft-only` 保留提案。普通 journal create/journalize 不强制调用标注；配置/显式参数决定自动接续。

## 原子采用

读取提案并校验正文/块，准备所有新概念说明；WriteTx 内核对目标 revision 仍是指定目标和 active annotation 仍为 expected；创建确切新 ConceptRevision、保存 binding/occurrence/path，切换该 revision 的 active_annotation，推进 namespace generation。

目标正文在模型执行期间变了：提案仍保存且指向旧正文；本次针对 current 的采用返回冲突。需要给历史版本标注时显式指定历史 ref 和 activate_for_current=false，不改变当前 generation。不要按字节相似把块搬过去。

重新标注只改 annotation head，正文 revision 不变化。正文新 revision 默认没有标注；正文完全相同也只在显式 reuse 并核对 body Artifact/parser profile 后复用位置，来源支持仍按新 revision 重新解释。

## 任务的位置

root blocks 的 B1/B2 构造由 annotation task handler 使用 TextView 完成。通用 InputPlan 不检查 task 名字。未知 key 是合同错误，不在 Compiler、CLI、Wave 再各校验一遍同一规则。

正式 Concept 身份属于 cognition；Wave 只消费已采用绑定。实验 Collection 的局部 Concept 仍可以运行，但不会反向写入正式 Concept 表。

同一目标版本上已经采用完全相同的 proposal 与 bindings 时返回原 AnnotationRef；新的绑定决定使用当前 active annotation 作为基准产生新标注。不要在重试时重复创建同一批新 Concept。

## 进程内与命令行使用

`runtime.annotations.propose` 接收 `known_concepts={"C1": exact_concept_ref}`；人工提案使用同一操作的 `proposal=AnnotationProposal(...)`，不需要模型配置。返回的 `result_ref` 可用 `read_proposal` 重新读取。`adopt` 接收目标正文、提案 ResultRef，以及可选的局部概念键到精确 Concept revision 的 bindings；null 明确新建。未覆盖的项按提案 reuse_key 或新 label/description 采用。

`journal annotate <ref> --proposal proposal.json` 接纳并采用人工提案；加 `--draft-only` 只保存提案。`--proposal-result res:...` 继续采用已保存的提案，`--known-concept C1=con:...@1` 提供模型候选，`--bindings bindings.json` 覆盖采用决定。`--historical` 明确给历史正文标注，保持当前检索代次。`journal annotation read ann:...` 读取正式绑定及位置，`journal annotation proposal res:...` 读取已保存提案。
