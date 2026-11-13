# 可编排检索与认知使用

本轮交付 Agent 的完整使用链：自然语言查询 → 精确原文 → 来源补读与关系深入 → 可选阅读整理 → Work 决定、实际使用和反馈。执行由现有普通异步 Runtime 持有，完成成果按正式引用读取。

## 所有权与工作依赖

共同材料先固定正式身份、领域属性、原文范围和关系。同一读事务捕获轻量目录，文本结构和计算准备在事务外完成。领域 owner 提供资格与事实，检索不再按 Note、Journal、Work 分别拼装私有目录。结果与文稿修订各自持久保存所选材料身份，读取直接消费已采用关系。

Wave 使用[程序与端口](../design/wave/program.md)组合[数值算子](../design/wave/operators.md)。观测、CandidateSet、CalibratedPool 和排名各自保留实际输入总体与尺度。表达式及情境在候选和排序阶段组合，最终只执行一次定位与总预算选择。模型评价沿同一个程序输入，保持其原始尺度和未评价状态。

[局部定位](../design/wave/localization.md)以实际原文单元构建词项和向量。TextView 提供句界、标题、列表、表格与代码范围；选择按原文并集计费，`delivery.read` 读出已选范围。目录与原文交付分别记录，预算未容纳的材料仍可继续读取。

[共同材料读取](../design/content/cognitive-materials.md)、[Work](../design/cognition/work-context.md)和[阶段情境](../design/cognition/context-routing.md)为认知查询提供领域事实。Work 保存目标、焦点、有效决定及精确依据；实际使用来自显式选区，查询反馈指向实际交付。相对时间以同一请求时刻绑定，锚定视图与当前视图分别执行。

[模型评价](../design/models/evaluation.md)与[Steward](../design/cognition/steward.md)复用模型连接、任务、期限和用量。Steward 先读取已保存 EvidenceDelivery，再形成独立 Markdown 与材料来源；通用 Markdown 转草稿和正式采用由写作 owner 负责。普通查询不强制存档，内存交付可在稍后通过 `retrieval.save_evidence` 采用。

## 入口与生命周期

CLI、SDK、本地宿主和[远程协议](../design/interfaces/remote-protocols.md)使用同一 Application。请求绑定、输入流、Progress/Notice/Saved、期限和取消保持共同语义；JSON Pointer 只选择输出字段。服务由显式宿主启动和关闭。

结果、原文、阅读稿、草稿和修订各有完成点。取消等待真实资源退出，保留已提交成果。执行中 lease、明确历史计算引用及反馈保护相应快照；普通保存的正文证据可独立于已清理计算资产继续读取。程序和读出参数变化复用未变内容及向量。

## 实验与默认选择

[实验研究](../research/composable-retrieval-study.md)记录固定家族划分、原始窗口、细单元、相同候选与预算、模型增量和实际成本。调参集与开发集用于选择程序、预算和阶段参数，保留集用于最终确认。原始数据与一次性脚本位于项目 temp，正式文档只保留结论及其口径。

默认优先选择直接可用的精确原文。阅读稿按用途调用，保持明确的来源和生成性质。后续领域依赖见[认知发展](cognitive-use-next.md)，该主题继续由各领域的真实形成、读取与采用操作推进。

## 使用验收

以真实项目材料运行查询、按 E 键读取、沿来源深入、跨进程继续 Work、采用决定并记录使用与反馈。分别检查普通预算与小预算、当前与锚定视图、取消和中断后读取、单项更新和清理，以及 SDK/CLI/宿主/HTTP/MCP/gRPC 的引用与原文一致性。运行记录保留实际请求、产物引用、原文范围、资源退出和模型用量。
