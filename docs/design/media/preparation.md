# 确定性媒体准备与有界批次

## 所有权

MediaPreparer 接收正式内容引用、模态选区、实际需求和配置，负责探测、解码、分段、采样、转换与确定性缓存。它不依赖 ModelExecutor、GenerationOperations、远端网络或完整 Runtime。

音频转写和媒体描述由 MediaAnalysis/GenerationOperations.transcribe 明确组织。没有对应能力时，准备返回所需表示或缺项，不在内部偷偷发起模型请求。

## 输入与输出

```text
inspect(content, settings) -> MediaInfo
plan(content, requirements, settings, options) -> MediaPlan
batches(plan, context) -> async context manager[async iterator[MaterializedBatch]]
text_view(content, options, context) -> TextRepresentation
```

MediaPlan 保存不可变原件 refs、请求范围、实际识别类型、输出表示要求、转换参数和轻量批次边界。每项 PlannedPart 明确：

```text
source: ContentSelection
source_media_type: 实际识别的 MIME
representation: text / image / audio / original_document / ...
target_media_type: 本次实际发送的 MIME
recipe: identity 或具体确定性转换参数
known_shape: width/height/duration/frames/pages（已知项）
```

这里的 MIME 来自探测及选定输出，不从文件名重新猜。原件接纳的 declared media_type 保留为输入信息；正常 PNG 改名为 .bin 或 .txt 不改变真实读取方式。计划未执行时不伪造输出 Artifact，identity recipe 可以直接引用原件；转换完成后给出实际 CAS Artifact 和已确认 MIME。

MediaPlan 的每个 PreparedBatch 保存 PlannedConversion 与 PlannedPart，转换完成后同一批值携带实际 InputGroup/parts 与 BinaryContent。`batches` 作用域按消费顺序物化；成功转换产物由 ResultStore/CAS 持久保存。视频按选定时段探测并 seek 解码，PDF 按当前页组转换；首批即可进入语言任务。

已确定的 batch 数或索引可以保存在内存；页数/帧数很大时使用紧凑边界或迭代器，不先解码完整输入。默认顺序消费；确有吞吐需要时有界预取，不建立新的任务调度器。

## 模态坐标

保留最新输入校验：文字仅行/字节，PDF 页码，图片帧/裁框、动画可含时段，音视频时段。当前视频不提供任意帧号/裁框操作；不适用字段明确失败。页码对外 1 基含端点，时段毫秒左闭右开。

实际 part 的 selector 属于实际 Artifact；原件选区通过 origins 表达。旋转、裁剪、缩放、抽帧和转写均保留变换来源。媒体时间不填进认知发生轴。

## 能力与表示

从真实源类型、模型明确接受的 MIME、可用编码器和允许的信息变换中选择确定性表示。格式优先级及质量来自配置；无可发送格式时在准备阶段返回 wanted/available 和原因。

`media.image.formats` 指定 PNG/JPEG/WebP 的选择顺序，`quality` 控制 JPEG/WebP 编码。JPEG 移除 alpha 使用显式 `jpeg_background` 颜色，未配置时报缺项。PDF 页图、视频帧和动画图片共用编码选择。原件直传携带探测得到的真实类型。原生 PDF 超过字节上限或指定页选区时，提取当前页组为独立 PDF；仍超限时继续按页拆分。视觉页图与页文字是同组不同 part。音轨按 `media.audio.formats` 与路线接受格式选择编码；无直接格式时要求已配置转写任务。

同一阶段 fallback 路线分别核对表示；两阶段可以分别使用不同但来源一致的确定性表示。缺少能力不能由交集为空后走默认 PNG 绕过。额外转写为明确语言阶段，preview 和普通 generation 准备不执行它。

## 缓存与工作生命周期

转换缓存键由原件身份、精确选区、算法/库版本和参数决定。检查缓存发生在重工作之前。保留 PDF 当前已提交页复用与同原件并发合并；可复用相同键的轻量协调能力覆盖图片/帧转换，不建设通用分布式缓存服务。

解析通过既有 Execution.blocking；PDFium 的进程锁在实际 worker 中持有。FFmpeg 由模态 owner 管理，取消后终止并 wait。CAS 路径通过 borrow_local 作用域提供，期间不得提前回收。

## 覆盖与部分结果

分批上限控制一次处理量，默认请求整份时全部批次都必须可达。采样明确标为 sampled，列出实际帧时刻，音轨排除仅来自显式选择。成功转换和语言阶段的正式结果在之后失败时仍返回引用；unfinished coverage 表达真正未完成的范围。

## 实际批次与可搜索文字

用已有 Execution 管理顺序消费和可选有界预取；关闭迭代器时关闭实际解码/子进程。PDF 按需取得页，视频单遍/合理分段顺序解码，不为每帧从头启动 FFmpeg。若容器格式必须扫描元数据，状态说明实际探测阶段，不承诺零等待。

text_view 为模型、Wave 和后续 Journal 提供指定材料的实际文字表示及来源。PDF 多页拼接、缓存和采用在此 owner 完成。TextRepresentation 提供 body、共同 TextView、输入组、精确字节范围到页文字的 mappings 与 coverage；无文本且未显式选择 OCR/语言解析时报告缺项。

文本准备直接读取指定选区的 ArtifactStore.text_view，并使用 TextView.chunks(max_bytes=...) 形成精确字节引用；媒体探测不整读正文来计算行数。共同正文 owner 负责 Markdown 分块与 UTF-8 字节边界。

缓存键包含源字节身份/接纳语义、精确选区、真实源 MIME、目标 MIME、转换器版本、尺寸与质量参数。不同 JPEG/WebP 表示不能共用错误缓存。媒体范围与来源规则见内容专题；联合多输入规则见 [输入规划](../models/input-planning.md)。

## 批次作用域

`async with preparer.batches(plan, context) as batches` 取得一个操作内迭代器；`async for batch in batches` 的当前 payload 在推进下一批或关闭外层作用域前有效。调用者只持久保存正式 refs，不长期保留 BinaryContent；迭代器负责真实解码与借用资源的关闭。首个可用批次即可交付消费，无需预先得到全部派生 Artifact。
