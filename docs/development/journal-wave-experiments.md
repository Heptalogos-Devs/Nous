# Journal 概念结构与 Wave 使用实验

本轮实验使用真实 Artifact、模型执行、Concept 标注和 Wave 快照。目标是理解结构如何改变召回，并建立以后 Journal 可直接复用的能力；不创建 Journal 生命周期或通用评测平台。

## 直接可使用的任务

包内已注册 [journal-draft](../../src/nous/models/resources/journal-draft/prompt.toml) 与 [annotate-concepts](../../src/nous/models/resources/annotate-concepts/prompt.toml) 两个正式模板资源，对应 `journal-draft` 和 `concept-annotation` 任务合同。

`journal-draft` 使用 joint 输入，输出 Markdown，保留用户给出的来源局部引用；实际生成血缘由 ResultStore 保存。`annotate-concepts` 接收一份 TextView，输入呈现由任务专属准备器给出 B1/B2 和原文；输出为小型 concept-annotation 合同。它共用 Compiler/Executor，不另造 LLM 客户端。

```sh
nous tools model run --task journal-draft --input "<材料A>" --input "<材料B>"
nous tools result read "<实际结果标识符>" --sources
nous tools model run --task annotate-concepts --input "<上一步Markdown产物标识符>"
nous tools result read "<实际标注结果标识符>" --sources
```

两个任务由用户现有配置选模型，包内不写固定模型名。未配置模型时仍能人工提供标注和运行已有向量快照。

## 清单接入

AnnotationSet 是普通结果产物。Wave 的集合输入可以显式引用其 annotation Artifact；assets owner 读取已验证的公开 AnnotationSet 并生成 occurrences/paths，CLI 不手写 JSON 关系展开。

清单中持久化完整概念说明，标注 local_key 在当前清单内映射到 Concept key。不得用标签名称自动合并同名不同义概念。手工 paths 与 annotation_ref 为二选一，冲突要调用者明确选择，不保持两套可独立更新的权威。

本包 annotation.json 是“单份示例正文”的合同例子，执行时从实际 content/标注结果取得正文标识符；B1/B2 是对应 profile 内的局部键。不要原样复制一个样例键去指向另一份正文。

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

## 重复使用真实材料

[检索实验脚本](../../scripts/retrieval_experiment.py)使用正式 SDK 批量接纳材料、生成草稿和运行查询。复制或编辑[实验配置示例](../examples/retrieval-experiment.toml)，按需要更换文本路径、Python 源码、媒体选段和问题；路径与 glob 均相对仓库根目录。来源的 `selection` 使用内容选区合同，可指定音视频毫秒范围、PDF 页码、文本行号和图片范围。示例中的 `temp/test-materials` 材料按本机实际情况选择。图片、音频和视频按所选模型路线准备，视频支持原生输入时直接使用视频。

先用 `nous init` 建立独立实例并配置模型和检索空间，再从仓库根目录运行：

```powershell
python scripts/retrieval_experiment.py --instance ./temp/work/my-experiment/instance --plan ./docs/examples/retrieval-experiment.toml --output ./temp/work/my-experiment/results ingest
python scripts/retrieval_experiment.py --instance ./temp/work/my-experiment/instance --plan ./docs/examples/retrieval-experiment.toml --output ./temp/work/my-experiment/results draft
python scripts/retrieval_experiment.py --instance ./temp/work/my-experiment/instance --plan ./docs/examples/retrieval-experiment.toml --output ./temp/work/my-experiment/results adopt "<草稿标识符>" --title "<标题>" --body ./temp/work/my-experiment/reviewed.md --annotate
python scripts/retrieval_experiment.py --instance ./temp/work/my-experiment/instance --plan ./docs/examples/retrieval-experiment.toml --output ./temp/work/my-experiment/results query
```

`draft` 可在命令后指定一个或多个配置中的任务名；不指定时运行全部任务。草稿保存在实例和 `drafts.md`，阅读正文与来源后才执行 `adopt`。`--body` 用审阅后的 Markdown 覆盖草稿正文，未修改时省略；`--annotate` 明确请求概念标注。采用失败不会丢失草稿。`query` 将可读结果写入 `queries.md`，完整分组、片段和精确引用写入 `queries.json`；遇到上游限流时，本轮继续执行词项查询并保留未执行的语义查询，恢复后可运行 `query --retry-failed`，只重试未完成和失败的查询。用 `nous read "<修订标识符>" --sources` 回读原文和来源。根据实际结果判断相关性，不将某个预定文件名或排名当成唯一正确答案。

`state.json` 记录本次导入、草稿、采用和实例身份。使用同一实例和输出目录重新运行 `ingest` 会跳过未变更文件，并按上次保存的精确版本修订已改变的来源；如果文稿已被其他操作修改，会报告版本冲突。材料中原有的 `source:` 链接作为原文示例保留，原文件始终可通过来源读取。输出目录属于特定实例和命名空间，不会被脚本删除或用于另一实例。模型配置和凭据由实例管理，素材与结果不需要进入 Git。
