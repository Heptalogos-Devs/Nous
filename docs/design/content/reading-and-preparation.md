# 共同内容阅读与成批正文准备

## 两项完整能力

内容 owner 为 Journal、草稿、结果、Wave 和未来 Steward 提供两项可复用能力：

```text
read_view_many(selections, representations=existing) -> ContentReadView[]
prepare_texts(requests, policy) -> PreparedTextBatch
```

`ContentReadView` 表达用户能读什么；`PreparedTextBatch` 表达算法或语言任务实际消费的精确文本。两者共用 Artifact、CAS、TextView、现有派生结果，不互相复制权威。

## 阅读多媒体来源

```text
ContentReadView (`content.contracts`):
  source: ContentSelection
  name?, media_type, byte_length
  body_text?: str                 # 真正是文本且范围可读时
  available_representations[]     # 已存在、覆盖明确的转写/描述/页图等
  export_ref: ArtifactRef
  coverage
```

文本返回选定原文；图片/音频/视频/PDF 返回原件及页/时间/区域、可导出引用、已有表示。没有可读表示也是正常的媒体视图，不尝试把二进制解为 UTF-8。表示由现有 inputs/parts/origins/outputs 查找，只选生产者在 parameters.representations 中按有名输出明确声明的已采用结果，并保留其实际覆盖；每项声明给出 kind，可用 sources 固定该输出的精确来源。转写、媒体描述、页图/媒体转换与确定性文本生产者声明实际表示，文稿写作和概念标注输出不进入已有表示。不能把另一页或较窄时段的描述冒充整个来源。

默认阅读不运行新模型、不偷偷转写，也不为缺少描述标成失败。用户需要新解释时显式调用媒体操作。原件不存在或存储错误仍返回实际错误。

RecordReader 和草稿的 `source_views` 按来源键保存有序 ContentReadView；ResultStore.read_view 的 `source_views` 与有名输出 `view` 使用同一形状。Wave 来源展开也使用该方法。CLI 只渲染 ContentReadView；无 PDF/audio/image 专属业务分支。程序可以持有正式引用，必要时通过现有 export/borrow_local 读取实际字节。

## 成批输入与内存所有权

在 content 增加一个完整批处理实现（可用 `content/preparation.py`），输入为一组精确文稿引用及其所需范围、分片政策。复用现有 text.py、citations.py、projections.py；不是另一个 Markdown 解析器。

```text
TextPreparationRequest:
  body: ContentSelection
  selections?: exact selection[]
  layout?: whole_and_complete_blocks(target_bytes)
  semantic_projection: source_link_labels | raw

PreparedText:
  origin_contents: ContentSelection[]  # 有序的真实原始选区
  view.body: ContentSelection          # 可读文字的精确选区
  view.text: str                       # 原始 UTF-8 正文
  text: str                            # 引用 label 投影
  mappings: TextMapping[]
  semantic_content() -> ContentSelection # 需要持久输入时采用
```

请求先按实际正文/表示分组。ArtifactStore 的 `get_many(refs)` / `get_many_in(tx, refs)` 保持输入顺序及重复引用，批量读取 Artifact 和正式 producer，并核验实例；ContentStore 的 `resolve_many_in` 批量读取 blob 元数据。一次短事务校验身份和取得描述，结束后再读文件。每份不可变正文在一次 build 中最多解码/解析一次；其 whole、局部候选、引用校验和路径裁剪都复用这份 TextView。

`TextView` 提供完整根块分片和精确切片能力，删除 cognition/retrieval 中自己的 `_selections`。嵌套块不能与父块重复形成意外覆盖。单个超长代码块/列表保持完整并标明超目标大小；不任意截断源码。whole 与唯一局部选区完全相同时，按同一文稿/同一选区消除重复候选。

同 bytes 可以复用纯解析结果；Artifact 身份、来源、Namespace 和正式修订仍分别保留。不能按摘要合并两次独立接纳。

## 原文投影与引用

引用去标记使用共同 CommonMark 解析结果。真正的 source 链接投影为其可见 label，正文原件保持原样。代码块中的示例、普通链接、图片说明及格式按现行合同处理。候选选区始终引用原文；投影不能覆盖精确来源坐标。

`project_citations` 增加基于已解析正文/已知 spans 的批量投影能力，`semantic_text` 单项入口调用同一能力。不要为每个 candidate 再完整 parse 一遍原文。

Lexical 冻结可以直接使用 PreparedText.text 和原件选区，不因没有改变语义身份的字符串投影就逐候选创建 Artifact/ProcessingResult。Native 索引本来保存可重建正文。发生真实 Embedding 需要精确实际输入时，由共同 projection/Embedding owner 批量复用或采用确定性文本表示，保存原文选区与转换签名；不让 Wave 手写模型输入来源。

`semantic_text_many` 对已采用投影批量查找缓存，不逐项先 find_cached 再 get 再 read。新转换先发布字节，再用 ResultStore 的成批采用能力提交；单项 adopt 复用同一内部实现。无需为无变换正文产生输出记录。

## 缓存边界

首要优化是批量读取与一次解析，不先建设通用缓存服务。

- 一个 build 持有明确的准备会话；候选与路径复用会话内已解析对象。
- 长期宿主可以使用 content owner 的有界 LRU，按 bytes 上限而非无界条目保留纯解析结果。
- CLI 每次重开时，可在已配置 `cache` 根复用序列化的纯 TextView 结构；键包含正文内容摘要、实际 parser/profile 与影响输出的选项。缓存不含 Artifact 身份或独立来源结论，重绑到本次正式引用。
- 该缓存是可删除重建的局部资产，失效/损坏时丢弃对应一项再解析；正式内容错误照常报错。没有旧格式兼容链。

cache 写入复用项目已有的原子文件发布能力；若基础尚未提供，补在共享文件能力并迁移同类写入，不各模块复制临时文件/replace 机制。配置 cache 容量和启停进入正式目录，默认值可直接使用。

多媒体正文表示仍由 MediaPreparer 产生。TextMapping 的 origins 从实际 InputPart.origins 取得；只有无派生 origin 的 part 才使用 part.content。PreparedText 和切片保留覆盖映射中的有序 origin_contents，不把多个原页/时间段替换成整个原件；非空文字缺少实际映射时返回 content.origin_missing。本次构建同一 PDF/媒体选区只调用一次确定性抽文/表示准备；content 不依赖模型。大量正文分组分批释放内存，不把整个实例所有字节永久缓存。

## 与文稿绑定和正文标题的衔接

来源 key 解析结果可以对应多个 ContentSelection，按实际顺序调用 read_view_many。非连续范围保持多项，不返回一段包含未提供内容的虚构原文。共同阅读只展开现有关系；同一个资料下有多份描述时返回其 task/result/coverage，使调用者明确选择，不自行选“最长”或“最新”解释为真值。

缓存预算由 ContentPreparation 所有者按现行配置消费；纯缓存条目由 schema/profile 不匹配直接失效，不新增内部 v1/v2 解码器。UTF-8 原文 slice 与语义投影文字分别返回，local edits 只能作用于原文，不拿去掉 source URI 后的投影偏移修改正文。
