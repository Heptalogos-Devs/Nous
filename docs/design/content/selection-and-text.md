# 无损正文、统一选区与批量准备

## 内容的权威

Artifact/CAS 保存不可变 UTF-8 原文或二进制原件；TextView 是派生结构。渲染 token、检索文本、段落键与语义投影都不能成为重新构造原文的唯一材料。

content 负责身份、范围读取、字节坐标、纯结构索引和已有表示读取；media 负责确定性图片/PDF/音视频表示；`application/preparation.py` 组织跨格式批量准备。content 不导入 MediaPreparer；MediaPreparer 可以调用 content 的原文能力。语言任务由 GenerationOperations 组织，确定性媒体层不调用模型执行器。

## 统一范围合同

```python
async def ArtifactStore.normalize_selections(values: Sequence[ContentSelection]) -> tuple[ContentSelection, ...]: ...
async def ArtifactStore.read_selections(values: Sequence[ContentSelection]) -> tuple[SelectedText, ...]: ...
async def ArtifactStore.text_views(values: Sequence[ContentSelection]) -> tuple[TextView, ...]: ...
```

SelectedText 包含实际 UTF-8 字节/解码文本、原件绝对起止字节、真实 first_line、不可变 artifact 与 blob identity。按行范围是 1-based inclusive，归一后的字节范围是 half-open。读取上限作用于请求返回的选区；扫描前缀可流式执行，不能为 11 字节范围先读取 4KB/更大的整件。

文本选择不能切断 UTF-8 字符；CRLF、空行、结尾换行、无结尾换行均按真实字节保留。字节选区的 first_line 不能固定填 1，按共同正文索引或流式前缀计算。规范化后的相交/包含关系由同一内容能力计算，解决行选区找不到已有表示的问题。

`subtract_selection` 计算指定选区中尚未覆盖的范围；`union_selections` 按原件与原文顺序合并重叠或相邻字节范围，保留不连续间隙。它们供证据交付、独立回读、来源关系和实际使用记录共同使用；复杂媒体坐标无法精确表示差集时保留待确认范围。

PDF页、图片帧/裁剪、音视频时间范围分别由媒体已知元信息验证，不混用字节范围。跨模态表示通过 TextMapping/OriginSlice 保留有序原始范围；范围不足时只能表达实际覆盖，不能扩为整件。读取已存在的表示不自动触发 OCR、转写或新模型请求。

## 已采用表示回原文

`ArtifactStore.map_origins(content, target_body)` 返回有序 `OriginSlice`，只沿表示生产者在结果采用时登记的有名输出映射变换范围。每项 origin 保留目标原件的精确坐标，transform_ref 指向正式变换结果。同字节的另一件原件、共同参与一次计算或任意血缘祖先都不建立坐标映射。

OutputMapping 绑定 output_name，保存输出字节范围、有序精确来源及 relation。identity 对等长 UTF-8 字节范围进行线性换算；atomic 的任意交叠返回完整的对应源单元；derived 记录处理来源，不提供精确坐标换算。CommonMark source-link 投影对未改字节登记 identity，对可见标签登记 atomic。拼接间隙或没有到目标正文的精确路径时返回 content.origin_unmapped。

范围变换可以保留多个不连续原文选区。认知来源 owner 再核对它们与指定正式 revision 的归属，模型 actual_inputs 仍保存实际消费的表示，不能以回映后的原文替换 part.content。既有 TextMapping 没有细粒度变换时，仅能使用其完整映射片段；读取不触发新模型或根据样本文字猜位置。

## 无损输入与结构输入

TaskDefinition 声明 `text_input_mode = lossless | structured`。全文 rewrite、edit_target、程序编辑的原稿采用 lossless。默认完整材料读取也保留完整所选范围；structured 可以暴露块键、标题路径，但不能默默丢弃块外内容。

lossless 输入直接从精确选区分配连续字节片段，按 UTF-8 与实际请求容量切分；所有片段按顺序合并应等于输入范围原文。输入不能容纳时返回明确容量错误或由任务自身定义分阶段语义，底层不分别摘要再假称全文。

CommonMark 的 reference definitions、仅定义行、块间空白、尾部内容没有可渲染 token 时仍是合法选区。TextView 持有完整 raw text/范围，结构索引只补充块边界。无损分片覆盖区间的空隙要保留；不能仅遍历根块后拼接。

模型输入中的 `BASE` 保持原目录和 source 链接；程序不事后猜回丢掉的定义。局部编辑同时区分 edit_target 与只读 context，程序对精确 base 应用 edits，范围外字节不变。纯文字展示、Prompt preview 和真实 HTTP request 必须消费同一准备结果。

## 结构缓存与内存

缓存键使用 blob digest、parser profile 及真实影响解析的选项。缓存只含纯结构信息、字节边界、标题路径与 citation spans，不包含 Namespace、ArtifactRef、RevisionRef 或独立证据结论。同样字节可以复用结构，但每次引用保留其自己的 Artifact 身份。

内存缓存持有已解析的紧凑结构，磁盘缓存保存同一结构的序列化形式；内存命中不重复 JSON 解码。结构重绑建立轻量 TextView，具体 TextBlock/BlockRef/片段文本按调用需要生成。一个 build 的多候选共享原文和结构，不能为每个候选创建一份完整正文或全部块。

读取缓存、JSON/结构验证、重绑涉及的全量计算、序列化、checksum 和 Markdown parse 全部在 ExecutionManager 管理的 blocking 边界内。缓存命中也必须服从该边界。只是把首次 parse 丢进线程不能解决 25,000 段缓存命中的同步阻塞。

LRU 按实际缓存容量有界；缓存损坏删除对应条目重建，正式内容损坏保留真实错误。沿用既有原子文件发布能力，不复制 temp/replace 实现。不要为此建设通用跨业务缓存服务。

## 批量准备与检索投影

`application/preparation.py` 的 TextPreparation 会话由 `prepare_many` 批量取得 Artifact/blob 元信息，按相同正文与所需范围合并重叠/相邻选区；合并不超过实际读取上限，也不包含离散间隙。已准备的覆盖视图与规范化选区共享缓存，后续 `prepare` 复用。完整正文确需参与 whole 候选时只读/解析一次；只需局部的请求不能强制全件读取。多个重叠选区共享结构与允许读取的实际范围，不用超出请求的整件读取换取缓存便利。

PreparedText 保留原文 TextView、实际 origin_contents 和 TextMapping；语义投影明确选择 raw 或 source_link_labels。检索 label 投影由既有 citations/projections 完成，正文写作不采用检索净化后的文本。Lexical 可直接使用可重建字符串；需要 Embedding 持久输入时复用 semantic_text_many 批量采用。

将 Wave 的私有范围读取、MediaPreparer 的文本分片和模型输入中的重复选择规则迁移到此共同能力。MediaPreparer 继续拥有真实格式协商、图像/PDF/音视频确定性处理；按格式责任整理现有巨大 preparation.py，保留一个公共准备入口，不按每个函数再造转发文件。

## 消费者迁移

ArtifactStore 的批量范围能力供 `read_view_many/read_selection/text_view/read_blocks`、模型输入、认知写作与标注、Wave、Embedding 及来源/反馈读取共用。`application/preparation.py` 组织跨格式批量准备；`wave/assets.py` 通过 prepare_many 预备候选范围，范围包含关系使用 normalize_selections。

## 完成行为

引用定义保留在实际 rewrite HTTP 输入；仅定义行可被选中并编辑。小范围在 read limit 内时能经 Artifact、Wave、模型三条通路读取。同行/字节选区关联相同已有表示。缓存命中的长文操作不阻塞事件循环，50ms 期限到达后不会因同步计算结束而报告成功；已在运行的线程仍等待实际结束后清理。whole/local 与 grouped retrieval 的既有召回质量合同保持。
