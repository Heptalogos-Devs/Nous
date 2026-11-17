# 正式 Concept 与意义版本

## 定义与必要能力

Concept 是有稳定身份的概念，包含可读 label、description、aliases 与不可变 revision。label 用于发现，完整描述用于既有 document 编码规则。不同概念允许相同名称；名称、Embedding 相似度和模型输出都不直接充当身份。

本轮实现 create/read/list/find/revise。暂不实现自动合并、拆分、层级推理或重定向 alias 链。术语别名是同一意义版本的描述字段，不是旧内部 API 兼容。

```text
create_concept(namespace, label, description, aliases=[]) -> ConceptRevisionRef
revise_concept(ref, expected_revision, label, description, aliases) -> ConceptRevisionRef
find_concepts(namespace, text, limit) -> readable candidates
```

find 先使用名称/别名及已有可用检索能力；没有向量索引时仍能做文字发现。不能为了找复用候选而要求先完成正准备构建的认知快照。

## 标注中的复用

标注输入可以带少量现有概念候选，以 C1/C2 局部键展示确切 label/description/revision。模型只提出 `use C1` 或新 label/description，不能构造正式 ID。无上下文候选时仅提出新概念。

采用可以由人工给映射，也可一次授权采用 proposal 的明确选择。领域 owner 校验所有复用键属于给定候选且版本存在；新概念在同一标注采用事务创建。没有命中并不默认把相似名称合并；歧义时保留提案供编辑，或明确新建。

不为每个 Concept 要求人工确认；自动采用取决于调用方选择的 annotation adoption。正常 Journal 保存完全不以标注成功为前置。

## 意义版本与当前图

正文标注固定 ConceptRevisionRef。概念修订不会无声改写旧标注。当前快照中的节点键使用 `(concept_id, concept_revision_id)`；若两篇当前文稿绑定不同意义版本，两个节点与各自向量同时存在，不把其中一个向量覆盖另一个。

逻辑 `con(…)` 查询在所选快照中解析为该身份的实际已绑定版本集合并展示版本；精确 `con(…)@N` 只选择该版本。请求快照没有该节点时给可读缺项，不猜别的版本。后续重标注可明确使用新意义版本；旧快照保持原解释。

概念的单独修订更新管理/未来候选定义；没有新的活动绑定或关系变化时不推进检索 generation，也不给已有文稿增加新节点。Namespace capture 只投影实际被当前标注使用的版本，以及本次明确采用的关系端点。

语义变化太大、作者认为是不同概念时直接 create 新身份，旧概念继续存在。这样的判断是正式认知决定，Native 只计算提供的版本。
