# 共同内容阅读与成批正文准备

## 两项完整能力

内容 owner 为 Journal、草稿、结果、Wave 和未来 Steward 提供两项可复用能力：

```text
read_view_many(selections, representations=existing) -> ContentReadView[]
TextPreparation.prepare_many(selections) -> tuple[PreparedText, ...]
```

`ContentReadView` 表达用户能读什么；`PreparedText` 保留算法实际消费的精确原文及语义投影。两者共用 Artifact、CAS、TextView、现有派生结果，不互相复制权威。

## 阅读多媒体来源

```text
ContentReadView (`content.contracts`):
  source: ContentSelection
  name, media_type, detected_media_type?, byte_length
  body_text?: str                 # 真正是文本且范围可读时
  available_representations[]     # 已存在、覆盖明确的转写/描述/页图等
  export_ref: ArtifactRef
  coverage
```

文本返回选定原文；图片/音频/视频/PDF 返回原件及页/时间/区域、可导出引用、已有表示。`media_type` 是接纳类型，`detected_media_type` 是已探测的实际类型；未知类型的 UTF-8 正文也可以直接读取。没有可读表示也是正常的媒体视图。成果采用时将各表示输出声明的 OutputMapping 来源写入索引，读取按该索引找到覆盖所选范围的表示；每项保留命名输出及生产者声明的精确来源。表示的坐标换算使用这些正式映射，见[选区与正文](selection-and-text.md#已采用表示回原文)。

内容读取、媒体准备和模型输出共用文本类型判断：`text/*`、JSON、XML 和 XHTML 按 UTF-8 文本处理，选区和原件身份贯穿各入口。

默认阅读不运行新模型、不偷偷转写，也不为缺少描述标成失败。用户需要新解释时显式调用媒体操作。原件不存在或存储错误仍返回实际错误。

RecordReader 和草稿的 `source_views` 按来源键保存有序 ContentReadView；ResultStore.read_view 的 `source_views` 展开用户所选来源，`input_views` 按实际输入组展开各表示，有名输出 `view` 使用同一形状。重复的精确选区在同次读取内复用，单项文本读取受配置上限约束。Wave 来源展开也使用该方法。程序可以持有正式引用，必要时通过现有 export/borrow_local 读取实际字节。

## 成批输入与内存所有权

`application/preparation.py` 的 `TextPreparation` 组织 content 与 media 的成批准备；`prepare_many` 接收有序 `ContentSelection`，`prepare` 复用该会话已准备的精确范围。正文机制由 `content/text.py`、`citations.py` 和 `projections.py` 提供，content 不导入 MediaPreparer。

```text
TextPreparation:
  prime()                          # 批量取得已知原件的 blob 元信息
  prepare_many(ContentSelection[]) # 合并实际重叠范围并准备
  prepare(ContentSelection)        # 复用整个正文或覆盖视图

PreparedText:
  origin_contents: ContentSelection[]  # 有序的真实原始选区
  view.body: ContentSelection          # 可读文字的精确选区
  view.text: str                       # 原始 UTF-8 正文
  text: str                            # 引用 label 投影
  mappings: TextMapping[]
  semantic_content() -> ContentSelection # 需要持久输入时采用
```

请求先按实际正文/表示分组。ArtifactStore 的 `get_many(refs)` / `get_many_in(tx, refs)` 保持输入顺序及重复引用，批量读取 Artifact 和正式 producer，并核验实例；ContentStore 的 `resolve_many_in` 批量读取 blob 元数据。一次短事务校验身份和取得描述，结束后再读文件。同一正文的 whole 请求只读/解析一次，局部候选复用其 TextView。只有局部请求时按规范化字节范围合并重叠或相邻区间，合并后的读取仍受 text_read_max_bytes 限制，离散区间分别读取。重复与已覆盖选区共享视图。

仅提供 `B1` 等字符串块键时，需要先读取完整正文并建立对应版本的 `TextView` 才能解析键；当前这一入口要求全文符合 `text_read_max_bytes`。已经取得的精确 `BlockRef.content` 或 `RevisionSelection` 可直接按其字节选区读取，读取上限作用于选中范围。`read_blocks` 需要验证块键与原视图的对应关系，因此也会读取其完整视图。

`TextView.complete_chunks` 优先按完整根块形成检索片段，同时覆盖块前、块间和尾部原文；超出目标大小的普通段落使用 UTF-8 安全边界切分，并保留完整的来源链接投影范围。单个超长代码块或列表保持完整。`chunks` 按实际容量形成连续无损 UTF-8 片段，`slice` 读取精确原文字节。全文 rewrite 与 edit_target 使用无损输入。嵌套块不能与父块重复形成意外覆盖。whole 与唯一局部选区完全相同时，按同一文稿/同一选区消除重复候选。

布局在加入下一结构块会超过目标时先封段。截取视图为布局保留相交源块的种类和范围；公开块引用仍只指向完整块，因此截断的代码不会被重新解析成普通段落或冒充完整可编辑块。

同 bytes 可以复用纯解析结果；Artifact 身份、来源、Namespace 和正式修订仍分别保留。不能按摘要合并两次独立接纳。

## 原文投影与引用

引用去标记使用共同 CommonMark 解析结果。真正的 source 链接投影为其可见 label，正文原件保持原样。代码块中的示例、普通链接、图片说明及格式按现行合同处理。候选选区始终引用原文；投影不能覆盖精确来源坐标。

`TextView.citation_spans` 保存共同解析取得的引用范围，`project_citation_slice` 使用这些范围生成局部投影；`semantic_text_many` 批量复用或采用需要持久化的投影，`semantic_text` 复用其单项能力。不要为每个 candidate 再完整 parse 一遍原文。

Lexical 冻结可以直接使用 PreparedText.text 和原件选区，不因没有改变语义身份的字符串投影就逐候选创建 Artifact/ProcessingResult。Native 索引本来保存可重建正文。发生真实 Embedding 需要精确实际输入时，由共同 projection/Embedding owner 批量复用或采用确定性文本表示，保存原文选区与转换签名；不让 Wave 手写模型输入来源。

`semantic_text_many` 对已采用投影批量查找缓存，不逐项先 find_cached 再 get 再 read。新转换先发布字节，再用 ResultStore 的成批采用能力提交；单项 adopt 复用同一内部实现。无需为无变换正文产生输出记录。

## 缓存边界

首要优化是批量读取与一次解析，不先建设通用缓存服务。

- 一个 build 持有明确的准备会话；候选与路径复用会话内已解析对象。
- 长期宿主可以使用 content owner 的有界 LRU，按 bytes 上限而非无界条目保留纯解析结果。
- CLI 每次重开时，可在已配置 `cache` 根复用序列化的纯 TextView 结构；键包含正文内容摘要、实际 parser/profile 与影响输出的选项。缓存不含 Artifact 身份或独立来源结论，重绑到本次正式引用。
- 该缓存是可删除重建的局部资产，失效/损坏时丢弃对应一项再解析；正式内容错误照常报错。没有旧格式兼容链。

TextStructureCache 的内存 LRU 保存已解码的纯结构，磁盘条目保存结构及 checksum；命中时轻量绑定本次 Artifact，具体 TextBlock/BlockRef 按需生成。读取、校验、序列化、Markdown parse 和绑定全部进入 ExecutionManager.blocking，取消后等待实际线程结束。cache 写入复用 common.files.write_text，容量和启停来自 content.text_cache。

多媒体正文表示仍由 MediaPreparer 产生。TextMapping 的 origins 从实际 InputPart.origins 取得；只有无派生 origin 的 part 才使用 part.content。PreparedText 和切片保留覆盖映射中的有序 origin_contents，不把多个原页/时间段替换成整个原件；非空文字缺少实际映射时返回 content.origin_missing。本次构建同一 PDF/媒体选区只调用一次确定性抽文/表示准备；content 不依赖模型。大量正文分组分批释放内存，不把整个实例所有字节永久缓存。

## 与文稿绑定和正文标题的衔接

来源 key 解析结果可以对应多个 ContentSelection，按实际顺序调用 read_view_many。非连续范围保持多项，不返回一段包含未提供内容的虚构原文。共同阅读只展开现有关系；同一个资料下有多份描述时返回其 task/result/coverage，使调用者明确选择，不自行选“最长”或“最新”解释为真值。

缓存预算由 TextStructureCache 按 content.text_cache 配置消费；纯缓存条目由 schema/profile 不匹配直接失效，不新增内部 v1/v2 解码器。UTF-8 原文 slice 与语义投影文字分别返回，local edits 只能作用于原文，不拿去掉 source URI 后的投影偏移修改正文。
