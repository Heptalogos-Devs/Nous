# 确定性媒体准备与有界批次

## 所有权

MediaPreparer 接收正式内容引用、模态选区、实际需求和配置，负责探测、解码、分段、采样、转换与确定性缓存。它不依赖 ModelExecutor、GenerationOperations、远端网络或完整 Runtime。

音频转写和媒体描述由 MediaAnalysis/GenerationOperations.transcribe 明确组织。没有对应能力时，准备返回所需表示或缺项，不在内部偷偷发起模型请求。

## 输入与输出

```text
inspect(artifact, settings) -> MediaInfo
prepare(artifact, selection, requirements, settings, options) -> PreparedMedia
materialize(PreparedBatch) -> PreparedBatch
```

PreparedMedia 包含不可变原件 refs、请求选区、分批边界、表示要求、转换参数和 coverage；不持有整份长文的所有 BinaryContent。确定性转换逐批发布到 CAS；PreparedBatch 仅保留实际 InputGroup/parts 和紧凑元信息。materialize 在发送前读取一个批次，调用者完成发送后释放其消息和字节。

已确定的 batch 数或索引可以保存在内存；页数/帧数很大时使用紧凑边界或迭代器，不先解码完整输入。默认顺序消费；确有吞吐需要时有界预取，不建立新的任务调度器。

## 模态坐标

保留最新输入校验：文字仅行/字节，PDF 页码，图片帧/裁框、动画可含时段，音视频时段。当前视频不提供任意帧号/裁框操作；不适用字段明确失败。页码对外 1 基含端点，时段毫秒左闭右开。

实际 part 的 selector 属于实际 Artifact；原件选区通过 origins 表达。旋转、裁剪、缩放、抽帧和转写均保留变换来源。媒体时间不填进认知发生轴。

## 能力与表示

根据明确 MIME/模态支持选择确定性表示。原生 PDF 可直接使用；超出一次字节上限时按页组成可发送文档或使用明确视觉页表示，不因模型声明 PDF 就强制整份读入失败。视觉页图与页文字是同组不同 part。

同一阶段 fallback 路线分别核对表示；两阶段可以分别使用不同但来源一致的确定性表示。缺少能力不能由交集为空后走默认 PNG 绕过。额外转写为明确语言阶段，preview 和普通 generation 准备不执行它。

## 缓存与工作生命周期

转换缓存键由原件身份、精确选区、算法/库版本和参数决定。检查缓存发生在重工作之前。保留 PDF 当前已提交页复用与同原件并发合并；可复用相同键的轻量协调能力覆盖图片/帧转换，不建设通用分布式缓存服务。

解析通过既有 Execution.blocking；PDFium 的进程锁在实际 worker 中持有。FFmpeg 由模态 owner 管理，取消后终止并 wait。CAS 路径通过 borrow_local 作用域提供，期间不得提前回收。

## 覆盖与部分结果

分批上限控制一次处理量，默认请求整份时全部批次都必须可达。采样明确标为 sampled，列出实际帧时刻，音轨排除仅来自显式选择。成功转换和语言阶段的正式结果在之后失败时仍返回引用；unfinished coverage 表达真正未完成的范围。
