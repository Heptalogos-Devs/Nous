# 共同能力收敛与 Journal 接入准备

## 本轮目标与范围

基于 Nous `56f2a18c7968fed1e5ae960530b37ebbf655f900`，完成已经暴露的正常操作缺陷和结构修正，使同一项能力在 CLI、进程内调用、媒体与 Wave 中使用同一个完整操作。

本轮交付：媒体真实类型与格式协商、逐批转换、多输入任务、Embedding 全入口批次与无关请求并发、可排错的操作/尝试观测、相关范围来源查询、Rust 检索核心与完整候选池接口、稀疏图与长序列行为、正文块与概念路径实验。

Journal、journalize、修订、时间视图的领域设计同时入库；本轮不实现正式 Journal CRUD、自动长期采用、Social、Dream 调度、DBOS、完整 NousQL、HTTP/MCP。正文结构和标注实验是真实复用能力，不假报为已完成认知系统。

## 设计入口

| 工作 | 合同 |
|---|---|
| 模块与迁移 | [操作所有权](../design/foundation/operation-ownership.md) |
| 媒体类型、转换与覆盖 | [媒体准备](../design/media/preparation.md)、[多输入规划](../design/models/input-planning.md) |
| 向量批次与存储 | [Embedding](../design/models/embeddings.md) |
| 真实请求与尝试 | [请求尝试](../design/models/request-attempts.md) |
| 来源与文本 | [来源读取](../design/foundation/provenance.md)、[正文结构](../design/foundation/text-structure.md) |
| 观测与用户使用 | [诊断入口](../design/interfaces/diagnostics.md)、logging/tracing/errors 专题 |
| Rust 核心 | [检索内核](../design/wave/engine.md)、[Native](../design/wave/native.md) |
| Journal 与 Wave | [Journal](../design/cognition/journal.md)、[投影](../design/wave/journal-projection.md)、[关系与支持](../design/wave/evidence-projection.md) |
| 后续时间机制 | [时间视图](../design/wave/temporal-views.md) |

## 先完成共同输入、存储与诊断合同

修改 `content/contracts.py`、`content/schema.py`、`content/store.py` 与媒体计划：给实际/待转换 part 明确源类型、目标 MIME、表示和选区；类型识别结果不再在物化时被扩展名覆盖。已有 SourceSelection 继续使用，通过共同构造与校验补齐用途，不另造 Wave 私有选区语言。

内容表按 [来源读取](../design/foundation/provenance.md) 拆出可索引的 input parts 与 origins，输入组重建仍由 ResultStore 完成。调整 `ArtifactStore.read_selection` 为范围读取，不为几行先读整份大文件。统一 Markdown 结构供模型选段、文本展示和 Wave 使用。

将 SQLAlchemy 表定义从 `models/embedding_store.py` 移入纯 schema 模块；组合根显式注册 CAS、content、embedding、wave 元数据并一次解析跨表引用。删掉借导入某服务顺带建另一领域表的约定。

观测共同层建立根 operation context 和 attempt 记录；修改错误呈现与日志 formatter，随后媒体、Embedding、Native 使用这些能力。基础 logging 仍可以独立运行。

## 接通媒体与语言任务完整使用链

`MediaPreparer` 形成轻量计划和按需转换作用域。修改 image/PDF/video/audio 实际转换者，目标格式由计划选定，原件直接发送也携带已识别 MIME。透明图到 JPEG 的背景合成、动画/音轨处理均由明确配置或请求决定；未确定时报实际缺项。

保留 PDFium 在真实 worker 内的锁、FFmpeg 终止与 wait、已完成转换复用、取消后资源收尾。视频按顺序解码；首批产生后可以进入模型，不先完成整部视频。

`GenerationOperations` 使用共同 InputPlan；两个短文件的比较在同一次任务中出现。普通任务缺省 joint，独立批处理必须显式选择或由任务定义声明。预览、probe、run 和媒体各阶段使用同一个规划/编译实现，删除入口中的第二套组合。

`PreparedAttempt` 保存实际路线、参数、表示和额度。结构修复留在原路线，网络/服务暂态失败才按既定策略切换；流已有可见输出后不切换。移除媒体 bytes/256 硬性上下文拒绝，真实字节/尺寸/时长限制仍执行。

## 全部向量入口迁入共同批次

修改 `models/embeddings.py` 的批次执行，`application/embedding.py` 的接纳/复用/采用，以及 CLI、Wave 全部消费者。run/ensure/probe 均遵守同一请求上限并保留原输入下标。

移除跨网络的 `_ensure_lock`。精确缓存读取不排在无关工作后；同一调用内精确去重，跨调用使用已采用缓存和事务唯一性；允许并发未命中的少量重复请求，不新建共享请求生命周期。每批成功在短事务内采用，全部结果按原顺序返回；错误返回已完成 refs 和未完成下标。

矩阵按 Artifact 分组，通过 CAS 的本地借用和 NumPy 只读映射取行，不复制整份 NPY 到 bytes 后再解码。空间绑定仍在首次成功采用中完成。

## 将检索业务移回 Rust 核心

从 `native/python/src/engine.rs`、`query.rs` 迁移资产组装、持久化、候选通道、融合、DTSC、反证和 Thinking 组织到 `native/retrieval`。PyO3 保留参数/数组转换、detach、对象持有及错误映射；VCP crate 保留数值闭包。

补齐类型化 CandidatePool、独立 observe/readout/think、显式查询向量与空间身份。search 调用同一组核心操作，不再在 readout 重跑召回。统一各通道启停、数量和合并上限；直接锚通过概念倒排取得候选。

移除 100 个 occurrence 后静默跳过整个序列的条件；空图仍允许可定义的概念融合，传播返回空活动。数值错误返回具体错误，Thinking 缺少记录与内部失败分别表达。DTSC 不以默认几何掩盖坏输入。

build 只产生完成的关闭文件与摘要，Python 同卷发布后 open 一次；短事务登记 current。查询期间 Arc 保持稳定。修正取消、错误类型和公开诊断，不新建快照迁移、任务回执或守护进程。

## Journal 可复用准备与有限实验

本轮实际实现以下内容（先完成上述修复，再接入实验）：

- 共同 TextView/BlockRef 与精确原文读取，替换媒体、选段任务内的重复 Markdown 分段。
- `journal-draft` Markdown 写作任务与专一的 `annotate-concepts` 标注任务，后者输出正文块上的概念出现与局部路径，保存为普通 ProcessingResult；不创建 Journal 或正式 Concept。
- Wave 支持有边界的多条候选路径、结构贡献的明确支持组，以及解释性关系的显式实验 profile。原始 VCP 路径保持可单独调用；单路径且无扩展时保持原数值定义。
- 用现有 tools、真实模型和内容存储完成 [概念路径实验](../development/journal-wave-experiments.md)。样本运行产物放 temp；不建立评测平台。

[Journal 首条使用链计划](journal-first-use.md)记录后续设计，不在本轮自动开工。时间严格视图及查询期时间门控在设计中明确，本轮仅用显式来源子集的快照实验，不提前实现任意历史查询系统。

## 文档和当前代码一起迁移

先读取交付包的 APPLY.md，应用现行文档的局部补丁和新增专题。当前仓库若已有后续补充，逐处合并，不恢复旧整文件。沿真实生产者、调用方、配置、Schema、包导出、示例和 Agent 使用说明迁移；删除旧内部字段、旧入口和专属支撑，不留 alias/兼容解码/双运行链。

已有源码索引在实际职责迁移后更新。CURRENT 只更新完成能力和剩余目标，不追踪讨论文件。保护本轮之外的用户样本、网关补丁和实验实例；重置数据只针对得到授权的开发实例。

## 工作依赖与可并行范围

内容值/来源 schema、请求上下文和语义 InputPlan 先确定；媒体与向量围绕共同合同分别实施。Rust 数值核心迁移可并行，Python Wave 接线等待新的公开读取/向量合同。最后完成用户呈现和同一通路上的真实使用。

每项完整职责由一个负责人贯通，不让两个执行者分别修改同一个内容模型。拆任务按实际数据和调用依赖，不按文件数、分钟数或逐测试循环切分。

## 完成条件

正常 PNG 的三个文件名、JPEG/WebP 限定、两文件比较、长媒体首批、5 条 Embedding 按 2+2+1、并发建库与交互查询、失败重试诊断、来源读取、Native 固定池读出、稀疏图、101 项序列、快照重开、多路径候选及原文回读都通过正式入口给出有意义的结果。具体情境见 [使用通路](../development/repair-walkthrough.md)。

完成判断来自实际结果和职责收敛。不要恢复 CI，不要求本地全量测试，不逐 Helper 增加测试；由项目所有者进行独立验证。开发者仅针对真实故障运行有关通路；有独立保护价值的数学边界或事务场景可以保留最小回归，不用测试迫使生产导出或双实现。
