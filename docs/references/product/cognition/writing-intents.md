# 创建、编辑、重写与跨篇反思

`WRITE_DRAFT` 使用判别请求表达四种写作目的；调用者通过 `adoption=draft|save` 选择只保存草稿或继续正式采用。

| 意图 | 主要输入 | 采用效果 |
|---|---|---|
| new | Note、原件及其精确选区、可选主题 | 新 Journal |
| edit | 精确基准、允许修改的选区、指令和可选材料 | 将提案组装为完整正文，再修订同一文稿 |
| rewrite | 精确基准和全文重写要求 | 完整正文修订 |
| reflection | 至少一份精确 Journal 和新主题 | 新 Journal，保留正式来源 |

共同来源解析固定用户选择的正式修订、离散选区及用途。new 不把已知 Journal 当作重复压缩的主输入；需要改写或跨篇理解时明确选择 rewrite 或 reflection。current_basis 必须指向精确正式修订。

`journal-draft`、`journal-edit`、`journal-rewrite` 和 `journal-reflect` 定义各自输出语义。共同生成执行模型、保存实际输入和命名成果；写作 owner 保存领域上下文并组装草稿。edit 模型只取得被选中的目标和明确上下文，完整基准由程序用于组装。引用键、逐字节映射、反馈依据与原来源由各自 owner 核验。

草稿修改使用 `REVISE_DRAFT` 创建新草稿，正式采用使用 `ADOPT_DRAFT`。新草稿保留原草稿和模型成果；基准变化时采用返回冲突，已保存成果不受影响。详细保存点见[草稿与采用](draft-adoption.md)，精确编辑见[编辑合同](exact-editing.md)。

明确的 correction 会使以旧修订为 current_basis 的文稿待复核；意见类别和措辞不自动决定更正。旧版反馈应用到新基准时，调用者提供明确映射，见[反馈处理](feedback-processing.md)。
