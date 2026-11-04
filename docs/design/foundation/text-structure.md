# Markdown 正文结构与精确选段

## 正式正文与派生结构

正文仍是普通 UTF-8 Markdown Artifact。`TextView` 是从该不可变正文产生的结构视图，所有正文块都指向该版本的实际内容。它由 `content/text.py` 提供，模型、Journal、Steward 和 Wave 共用。

```text
TextView:
  body: ContentSelection
  parser_profile
  blocks[]: {key, kind, source_range, heading_path, parent_key?}

BlockRef = body ArtifactRef + parser_profile + local block key
read_blocks(refs, preserve_context=True) -> exact source selections
```

local block key 如 B1/B2，只在这份正文和解析 profile 内有效。公开长期引用固定 Artifact/JournalRevision，再附块键；正文修订后重新解析，旧键仍指旧版本。此轮不建立跨编辑永久段落追踪。

## 解析

使用现有 markdown-it-py 的 CommonMark token/source map，不按空行盲切，不从 HTML 渲染结果逆向恢复原文。行位置转换为原始字节范围时保留 UTF-8、CRLF、末尾换行和实际源文字，模型选段返回原字节解码结果。

根正文块保持不重叠：普通段落、代码块、列表、引用、标题等有自己的语法范围。列表和引用作为容器保留必要上下文；确需细分时提供明确子块与父块引用，不让阅读者拿到失去列表/引用语义的碎片。代码 fence、标题路径、引用祖先都可用于阅读上下文。

向量化使用正文及确定的标题上下文，不将 B1、行号、UUID 注入语义文字。模型看见的局部键由呈现器附加，正式正文不改写。

## 选择与分片

`read_selection` 对行/字节选区做实际范围读取。按行需要扫描前缀时可以流式扫描，不为十行读入整份文件；重复读取的行索引可以作为可重建缓存，以实际需要决定。

检索片段按章节/完整块合并到配置的输入目标；超过模型限制的单块由明确切片策略处理并保留父块、字节范围和覆盖。文章身份不等同于向量片段身份。一篇 Journal 可以只有一个向量，也可以有多个局部向量。

选段任务返回块键/范围。程序校验键属于本次提供视图并按原文取回，不让模型复述。去重按同一精确块/选区执行；不同来源的相同文字保留来源身份。

## 与媒体协作

PDF 原件通过 MediaPreparer.text_view 得到实际文字 Artifact、页范围及来源；TextView 只解析那份文字，不导入 PDFium。OCR/转写属明确语言或专用解析阶段，不能在普通文本读取中自动发生。

本轮先把现有 select-passages、媒体文字展示和 Wave 片段全部迁到 TextView，再让 Journal 标注实验使用。缓存由正文身份与 parser_profile 确定，无需新增正文数据库权威。

依据：[markdown-it-py token/source map](https://markdown-it-py.readthedocs.io/en/latest/using.html)。
