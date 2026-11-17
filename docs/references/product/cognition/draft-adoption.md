# 完成写作草稿与正式采用

new、edit、rewrite 和 reflection 使用共同 `WRITE_DRAFT` 操作。每次生成先捕获精确来源及必要基准，语言任务形成完整输出后，将模型成果与 `WritingProductContext` 一起保存。该成果记录模型实际收到的输入；edit 的完整基准由后续程序组装使用，不写入模型输入。

`!NO_MATERIAL!` 经任务解释为无材料时仍保存模型成果，不创建空草稿。正常输出按任务形成完整 Markdown；引用键错误也可保存在草稿中，正式采用前由领域规则检查。

## 草稿组装和修改

Draft 保存模型成果、完整正文、来源、写作意图和精确基准。局部编辑的 `writing.compose` 程序结果保存完整组装及映射，未选中的原文字节、CRLF、引用定义和尾部保持原样。模型成果已保存而组装失败时，可通过 `DRAFT_FROM_RESULT` 对该成果发起新的本地组装，不再次请求模型。

`READ_DRAFT` 读取草稿及可选来源；`REVISE_DRAFT` 接收完整 Markdown 和可选 `SourceChanges`，产生带 parent 引用的新完整草稿。旧草稿、模型成果和采用前人工文件继续可读。修改引用目录需与实际正文一致。

## 正式采用

`ADOPT_DRAFT` 采用指定的完整草稿。new/reflection 创建新 Journal；edit/rewrite 在事务中核对原精确基准和当前 head。正文、来源、相关反馈处理及草稿采用关联同事务提交；冲突保留草稿和已完成成果。用户可基于新的精确正文发起新修改。

再次采用同一草稿返回已有修订，不再次修改正文或启动维护。需要另建一篇时显式选择 fork。`adoption=save` 将草稿保存与正式采用接续执行，返回草稿和正式结果。正文提交后的标注与刷新见[维护架构](maintenance-architecture.md)。
