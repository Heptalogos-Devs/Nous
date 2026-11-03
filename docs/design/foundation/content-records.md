# 内容、输入表示与来源记录

## 正式对象

Artifact 表示一次正式接纳或生成的不可变内容，拥有稳定身份、短地址、MIME、名称及可用时间。BlobRef 仅表示字节摘要和长度。同样字节的不同接纳可以属于不同事件；推导同一份材料的多个版本也不会增加独立证据。

ProcessingResult 保存成功处理的 operation、producer、精确输入、有名输出、参数、coverage、formed_at、recorded_at。失败或中断由用例返回部分结果，不先建通用 running/receipt 总表。

`recorded_at` 与实际接纳/生成时间使用组合根提供的 Clock.utc_now。认知事件的其它轴以后由实际领域提供，不用接纳时刻补造接触或发生。

## 精确位置属于被指向的正文

```text
SourceSelection:
  pages: tuple[int]
  start_line/end_line: int?    # 1 基、含端点
  start_byte/end_byte: int?    # 半开；与行选区互斥
  start_ms/end_ms: int?        # 半开
  frame: int?
  crop: (left,top,right,bottom)?

ContentSelection:
  artifact: ArtifactRef
  selection: SourceSelection  # 空选区表示 Whole

OriginSlice:
  origin: ContentSelection
  transform_ref: ResultRef?     # 精确指出产生当前表示的处理

InputPart:
  content: ContentSelection     # 这次实际读取/发送的字节或文字位置
  representation: original | text | image | audio | transcript | ...
  origins: tuple[OriginSlice]   # 原件位置，允许多个

InputGroup:
  key: str                     # 模型局部键，如 S1:P3
  name: str
  parts: ordered tuple[InputPart]
```

SourceSelection 按模态验证可用坐标，模型、媒体、Embedding 与 Wave 共用 ContentSelection。一个组可以同时提供页文字与页图，每个 part 各自有正式 Artifact。局部 key 指组，不把两个不同正文强塞到一个 artifact 字段。

PDF 第 3 页的 PNG，其 `content` 是该 PNG 的 Whole；`origins` 指原 PDF 的 PdfPages(3)。音频 60–120 秒的 WAV，其 content 是分段文件 Whole/0–60 秒；原始时段只出现在 origins。文本片段按原 Artifact 行范围引用；选段回取这些行，不重新生成文字。

## 来源关系

结果记录两类用途：实际消费的有序 InputGroup；只用于解释派生血缘的 origins/处理关系。原件作为来源存在一次，实际使用整份原件时可以同时承担输入角色，但不再插入无类型的重复 `S1` 行。

`ResultStore.provenance(refs)` 批量读取有向无环的来源关系，按请求 Artifact ID 返回终端来源身份集合；Wave 消费这个公开能力，不遍历 content 的私有表。相同终端来源去重，派生深度和使用次数不扩大独立支持。没有外部来源的主动创作，其原始 Artifact 是一个明确的来源；查询上下文和算法计算不能被当成新的事实支持。

外部声明“这两个 Artifact 是同一真实来源”由输入清单/正式领域明确表达，不能仅凭 digest 推断。

## 存储与采用

CAS `owner/slot → BlobRef` 是正文唯一映射。Artifact 表不复制另一份可更新 digest。输入表按 ordinal 保存完整 InputGroup JSON，内含 key、part 顺序、实际 content、origins 和 representation；origin artifact/result 的存在和实例归属在采用中检查。输出 Artifact 由同一结果所有者建立。

`prepare_outputs` 发布字节；`adopt_in(tx)` 提交结果、输入关系、输出 Artifact 与 CAS 引用。上层使用同一个 WriteTx；读取完整结果和来源尽量批量完成，避免逐节点 SQL。

发布成功而 SQL 失败的字节可以被之后显式清理。没有启用 GC 时不要建设租约平台；今后 GC 需避开正在发布/采用的内容。

## 对外使用

ArtifactStore 提供 accept(InputSpec)、ingest、get、read_selection、read_text、export、borrow_local。read_selection 返回精确 UTF-8 全文、行或字节片段；其它模态坐标经确定性媒体准备取得实际表示；任意字节导出由 CAS/export 负责。ResultStore 提供采用、get、read_view、provenance。CLI 不自行恢复组/来源、串接多层私有字段或按输出位置猜测 text/image。输出通过名称取得，不能依赖 `outputs[-1]`。

开发实例使用当前 schema 重新初始化；没有旧输入表的兼容读写路径。
