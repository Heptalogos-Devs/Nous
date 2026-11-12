# 内容、输入表示与来源记录

## 正式对象

Artifact 表示一次正式接纳或生成的不可变内容，拥有稳定身份、可读标识符、接纳时的类型声明、名称及可用时间；经过探测的实际表示类型由 MediaInfo/InputPart 明确给出。BlobRef 仅表示字节摘要和长度。同样字节的不同接纳可以属于不同事件；推导同一份材料的多个版本也不会增加独立证据。

ArtifactRef 与 ResultRef 在序列化时分别携带 `kind=artifact` 与 `kind=result`；混合依赖在读取和传输后仍保留各自的引用类型。

ProcessingResult 保存成功处理的 operation、producer、精确输入、有名输出、参数、coverage、formed_at、recorded_at。失败或中断由用例返回部分结果，不先建通用 running/receipt 总表。

`recorded_at` 与实际接纳/生成时间使用组合根提供的 Clock.utc_now。认知事件的其它轴以后由实际领域提供，不用接纳时刻补造接触或发生。

## 精确位置属于被指向的正文

共同选区使用 `SourceSelection` 的页、行、字节、媒体时间、帧和裁剪字段；各入口直接使用同一值。页、文本与媒体坐标不混用；动画可同时使用帧、裁剪和时间范围。输入已有范围时，通过 `restrict` 取得其内的绝对子选区，空选区表示整份已声明范围。

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
  precision: identity | atomic

InputPart:
  content: ContentSelection     # 这次实际读取/发送的字节或文字位置
  representation: original | text | image | audio | transcript | ...
  media_type: str              # 本次实际发送/读取的已确认表示类型
  origins: tuple[OriginSlice]   # 原件位置，允许多个

InputGroup:
  key: str                     # 模型局部键，如 S1:P3
  name: str
  role: source | reference | current_basis | inspiration | revision_base | edit_target | context
  parts: ordered tuple[InputPart]
```

SourceSelection 按模态验证可用坐标，模型、媒体、Embedding 与 Wave 共用 ContentSelection。一个组可以同时提供页文字与页图，每个 part 各自有正式 Artifact。局部 key 指组，不把两个不同正文强塞到一个 artifact 字段。

PDF 第 3 页的 PNG，其 `content` 是该 PNG 的 Whole；`origins` 指原 PDF 的 PdfPages(3)。音频 60–120 秒的 WAV，其 content 是分段文件 Whole/0–60 秒；原始时段只出现在 origins。文本片段按原 Artifact 行范围引用；选段回取这些行，不重新生成文字。

## 来源关系

结果分别保存有序原选择 `SelectedInputs`、实际消费的 `InputGroup` 和映射及依赖。输出声明名称、角色与表示种类；程序组装使用完整基准，不改变模型实际输入。identity、atomic 和 derived 关系分别表达精确字节对应、完整源区间和不可逆派生。

`ResultStore.provenance(refs)` 批量读取有向无环的来源关系，按请求 Artifact ID 返回终端来源身份集合；Wave 消费这个公开能力，不遍历 content 的私有表。terminal_roots 保留完整血缘；support_roots 只沿 source/reference/current_basis 输入传播，inspiration/revision_base 保留引用但不贡献支持根。原始 Artifact 的支持根为自身；仅依赖启发或版本基准的结果没有新增支持根。相同终端来源去重，派生深度和使用次数不扩大独立支持。没有外部来源的主动创作，其原始 Artifact 是一个明确的来源；查询上下文和算法计算不能被当成新的事实支持。

外部声明“这两个 Artifact 是同一真实来源”由输入清单/正式领域明确表达，不能仅凭 digest 推断。

## 存储与采用

CAS `owner/slot → BlobRef` 是正文唯一映射。Artifact 表不复制另一份可更新 digest。输入组、实际 parts 与 origins 按 [来源读取](provenance.md) 规范化保存；公开 InputGroup 由唯一正式数据重建，不再维护第二份可更新 group JSON。origin artifact/result 的存在和实例归属在采用中检查。输出 Artifact 由同一结果所有者建立。

`prepare_outputs` 发布字节；`adopt_in(tx)` 提交结果、输入关系、输出 Artifact 与 CAS 引用。上层使用同一个 WriteTx；读取完整结果和来源尽量批量完成，避免逐节点 SQL。

发布成功而 SQL 失败的字节可以被之后显式清理。没有启用 GC 时不要建设租约平台；今后 GC 需避开正在发布/采用的内容。

## 对外使用

InputReader 接纳外部输入与已有内容；ArtifactStore 提供 ingest、get、精确读取与导出。ResultStore 提供采用、get、read_view、provenance。`READ_RESULT`、`READ_ARTIFACT` 和 `READ_SELECTIONS` 使用共同内容视图；输出通过名称、角色与表示种类读取。

开发实例使用当前 schema 重新初始化；没有旧输入表的兼容读写路径。

## 读取范围与共同正文结构

provenance 只查询请求引用的相关祖先，必要路径按需返回。内容血缘与认知独立支持的区别见 [来源读取](provenance.md)。正文行/字节范围读取不先加载整份大文件；Markdown 块与选段统一使用 [TextView](text-structure.md)。
