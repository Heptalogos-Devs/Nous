# 文本、Markdown 与 PDF 准备

## 文本与 Markdown

保留原始 UTF-8 正文，读取时不做隐式摘要。Markdown 的局部段落/语义块使用 markdown-it-py source map 定位，显示或选段从原文切片返回；代码块、列表和引用结构保持完整。

超长输入按语义块分批，标题可作为背景重复提供，但标记为背景而不当成多个新来源。每批结果绑定实际输入。

## PDF

优先复用 pypdfium2/PDFium 取得页数、页面文本和栅格图。可直接发送 PDF 的协议/模型由能力声明选择原生输入；通用路径使用页文本与所需页图，扫描页或图表页有视觉输入。

`document_mode=auto|visual|text` 对应 CLI 的 --document-mode。auto 优先采用当前协议真实支持的原生 PDF，否则使用所选页图与文字；visual 明确使用页图路径；text 只处理可提取文本并注明 text_only。某页没有可提取文字不当作空白页。缺少视觉能力时报告具体缺项或接受用户明确的 text_only，不静默完成一份不可读的扫描文档。

本轮不建设 OCR 工作流；需要理解图片文字时使用已配置视觉模型。页面处理按 max_pages_per_request 分批，输出保留实际页码与完整请求范围。

同一原件的并发页面准备协调缓存读取和转换；已保存的页按原件、页码、渲染模式、分辨率和工具版本复用，只有缺失的页面重新解析。不同请求仍各自保留自己的完整选区和覆盖信息。

## PDFium 的真实执行边界

PDFium 不支持多个线程同时调用，连不同文档也受约束。其 owner 内用进程级 threading.Lock 包住一个完整的打开/读取或渲染/关闭过程，锁在实际 worker 内取得和释放。不得仅用 async Semaphore 包住可提前取消的等待。

PDFium 句柄不逃出该过程，全部关闭后才返回纯文本、图片字节或临时文件结果。该约束不传播到所有媒体，Pillow/FFmpeg 使用各自成熟能力。

## 返回

所有页图/提取文本都有原 PDF 与页码来源。描述图表与原始页面读取分开，模型生成的解释不能替代原件。

依据：[PDFium 线程与资源约束](https://pypdfium2.readthedocs.io/en/stable/python_api.html)、[Markdown source map](https://markdown-it-py.readthedocs.io/en/latest/api/markdown_it.token.html)。
