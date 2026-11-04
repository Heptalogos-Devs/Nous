# Journal 概念结构与 Wave 使用实验

本轮实验使用真实 Artifact、模型执行、Concept 标注和 Wave 快照。目标是理解结构如何改变召回，并建立以后 Journal 可直接复用的能力；不创建 Journal 生命周期或通用评测平台。

## 直接可使用的任务

包内已注册 [journal-draft](../../src/nous/models/resources/journal-draft/prompt.toml) 与 [annotate-concepts](../../src/nous/models/resources/annotate-concepts/prompt.toml) 两个正式模板资源，对应 `journal-draft` 和 `concept-annotation` 任务合同。

`journal-draft` 使用 joint 输入，输出 Markdown，保留用户给出的来源局部引用；实际生成血缘由 ResultStore 保存。`annotate-concepts` 接收一份 TextView，输入呈现由任务专属准备器给出 B1/B2 和原文；输出为小型 concept-annotation 合同。它共用 Compiler/Executor，不另造 LLM 客户端。

```sh
nous tools model run --task journal-draft --input <材料A> --input <材料B>
nous tools result read <实际结果地址> --sources
nous tools model run --task annotate-concepts --input <上一步Markdown产物地址>
nous tools result read <实际标注结果地址> --sources
```

两个任务由用户现有配置选模型，包内不写固定模型名。未配置模型时仍能人工提供标注和运行已有向量快照。

## 清单接入

AnnotationSet 是普通结果产物。Wave 的集合输入可以显式引用其 annotation Artifact；assets owner 读取已验证的公开 AnnotationSet 并生成 occurrences/paths，CLI 不手写 JSON 关系展开。

清单中持久化完整概念说明，标注 local_key 在当前清单内映射到 Concept key。不得用标签名称自动合并同名不同义概念。手工 paths 与 annotation_ref 为二选一，冲突要调用者明确选择，不保持两套可独立更新的权威。

本包 annotation.json 是“单份示例正文”的合同例子，执行时从实际 content/标注结果取得正文地址；B1/B2 是对应 profile 内的局部键。不要原样复制一个样例键去指向另一份正文。

## 同一正文上的结构对照

先由人写一份合理的概念路径，保持正文、向量空间、候选池、其他参数相同，比较 memberships-only、原顺序、一条错序路径和多条局部路径。查看命中的正文、chosen_path、B/P/G/A、实际传播和 ranking，不只比较一个总分。

反向或打乱路径只是用于分辨是否真正使用顺序；不能预设每个查询的正确路径都更高。查询需要直接事实时 Dense 可能已经足够，关系型查询才可能受益。

## 来源与新解释

原始 A/B 形成 Journal C，C 再被总结为 D。C/D 保留同一共同依据组，重复次数不增加独立 witness；同时允许 C 形成一项此前没有的新解释关联。在 interpretation_gain=0 与显式非零 profile 下比较是否产生有用跨主题候选。

对照两个真正独立材料各自支持同一关系，以及两个材料共同才能推出该关系。前者是两个组，后者是一个 joint 组；候选含其中一个共同根时，joint 组不算完全独立支持。诊断同时显示实际比例和 VCP 衰减权重，0.15 底值不作为支持证据。

## 多路径与闭合

用一篇含“经过”和“机制”两个部分的文稿，分别标注局部路径；不能因它们连续出现在文件中就新建跨节关系。比较只有一条、两条真实路径和复制十次同一路径，观察 chosen_path 和独立支持是否符合语义。

再做“正确概念但正文没有谈到”的反例，检查正文闭合怎样限制奖励。程序校验引用存在，概念是否真正有语义依据通过读原文判断。

## 时间视图的起步

建立“当时可见材料”和“后来新增认识”两组明确快照，查询当时与现在的问题。历史视图不包含后来才形成的关联/Concept 意义；同一事件旧 occurred、晚 recorded 的材料在两种查询下分别解释。

本轮用人工确定的来源子集，不宣称已有 $asof 引擎。保留时间门控为未来设计，先观察源视图是否正确。

## 结果使用

若结构没有改善，保留实际结果，判断是标注、概念空间、候选通道还是读出造成；不增加更多 Prompt 限定或更复杂数学来掩盖输入问题。先让人工合理结构成立，再评价模型是否能稳定提出这种结构。

实验产物留在用户授权的 temp 实例，可用正式 read 查阅。无需固定案例数量、排名通过率、基准发布或永久运行日志。
