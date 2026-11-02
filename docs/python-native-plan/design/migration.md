# 迁移与数据边界

Python 应用拥有认知、来源和事务权威；同进程 native 只使用已授权的计算输入和可重建索引。旧 Rust 应用保留为研究参考；按实际需要吸收算法思路或纯计算。具体来源及落点见[源码采用](adoption.md)。

## 导入、来源与恢复

材料原件、输入目录、冻结 Unit、真实 Import Observation、UnitOutput、文件成功事实和后台 Job 各自保留身份。`UnitOutput` 指向实际 Observation；文件成功由实际 Unit 和 Output 的完成事实确定。文件去重按 scope、origin、policy、key、内容与仍可用结果判断，已接纳的成果可以跨作业复用。恢复历史成果不创建未发生的 Job 或模型调用。

`ImportDocument` 接受 text、markdown、html、pdf、json、jsonl、media、parquet、sqlite 和 graphml。SDK 文件及目录导入按文件类型上传原件；media 使用现有图像、GIF、音频和视频解释路径，`media_strategy` 可选择 native、description 或 description_then_native。HTML 由 Trafilatura 提取正文与表格；PDF 从已上传原件逐页读取文本层。提取结果保存在正式 `DerivedRepresentation`，Unit 的 `source` 指向精确派生片段，原始 HTML 或 PDF 页面另有原件区域。无文本层的 PDF 页保留页面 Unit，处理时通过已配置的媒体模型形成转写和描述，付费结果继续保存在同一派生表示体系。关闭模型处理时，扫描页返回需要 OCR 的具体错误。

Parquet 由 PyArrow 分批读取，GraphML 由 NetworkX 读取；MaiBot SQLite 在调用端通过 SQLite backup 捕获一致快照，再上传原件。原始记录中的明确实体、三元组和 Fact 分别交给原生形成、图和 Fact owner，来源外部标识按数据集限定。导入的向量保留在原件并检查其格式与维度，目标向量由当前受信模型路线生成。数据集导入使用 SDK `import_dataset`、CLI `nous dataset import`、MCP `import_dataset`；`export_bundle` 完成后返回可下载的原生 Bundle。A-Memorix 的 OpenIE、LPMM 转换和 MaiBot 迁移入口使用同一目录与持久任务。`backfill_source_times`／`nous dataset source-times` 只从已保存的明确时间字段补齐未知时间，可先预览。

LPMM `graph_structure.pkl` 在迁移边界作为原始归档上传。受限读取仅接受 NetworkX 的四种正式图类、其缓存视图和必要集合类型，再通过 NetworkX 转换为标准 GraphML；未知全局对象和外部对象引用直接拒绝。原 pickle 保留为不可变材料，转换后的 GraphML 依赖该原件，并进入同一目录、来源生命周期与 Bundle。

实际接触使用 `ImportService.contact`，HTTP 为 Subject 下的 `materials/contact`，SDK 为 `contact(namespace_id, subject_id, ContactMaterial(...))`。消费者身份来自已授权请求上下文。相同调用者、主体和 `external_event_id` 返回同一次 Observation；再次阅读使用新的外部事件身份，原件字节和证据根继续复用。文本接触产生 experiential Memory，资料上传只保留可访问原件，知识导入产生 declarative Memory。`form_long_term` 为实际接触提交持久形成任务；原始媒体复用媒体 owner 的解释与采纳路径，`media_strategy` 控制解释方式，形成的 Memory 保留本次接触身份。形成事务调用统一成长入口。`related` 指定本次接触的认知上下文，重新授权后随接触保存并交给成长入口。

原件、派生表示和派生片段统一通过材料 owner 的 `accepted_source_in` 读取接纳时的来源；延后形成、接触重放和查询均重新核验同一精确选择及其权限。处理 origin 变化时，新的 Observation 保留前一次接纳身份，延后形成继续读取原来选定的材料。

图片以 Material 保存原件，实际接触、区域和描述作为对应 Observation 与派生表示保存。媒体检索和向量投影使用同一来源授权与已配置的模型空间。

认知与 Fact 的证据变化由相应领域 owner 执行，后台形成与向量作业使用共同的任务执行平台。

`ImportDocument` 与结构化 `ImportRecord` 接受独立的 `event_time`、`observed_at` 和 `validity`。记录级显式值覆盖文档默认值；显式 `validity=null` 保留未知适用范围，省略该字段则允许形成时推断。接纳、冻结 Unit、文本与媒体形成共同保留这些区别，实际记录时间由本次事务确定。相同正文带有不同显式时间输入时分别处理。

来源撤回按实际支持关系影响认知，剩余独立来源决定对象是否继续有效。恢复和清除通过各数据 owner 更新认知、派生内容、上下文和索引；备份或 Bundle 不应重新引入已清除的正文。无 offset 的日期保留实际精度及时间域，不由导入调用者补猜 UTC。

纠错的新正文由本次实际支持来源支撑，旧正文及其支持保存为历史边界。传入 `PlanCorrection.supports` 时，操作说明仅记录纠错意图；直接用户纠正可由该次实际用户输入支撑。撤回纠正来源会使依赖它的新正文失效，历史修订仍可精确追踪。回滚恢复原修订的实际支持关系，纠错操作说明只留在谱系中。Fact 内容变更由 Fact owner 负责；共同生命周期可对相同内容更新状态。

Fact 的无引用断言按正文和精确适用窗口确认已有 claim；同一事实在不同窗口保留独立 current 对象及各自支持。冲突只发生在内容不同且适用期重叠的 claim 之间；不同时间域保留各自的适用判断。撤回和恢复改变认知状态，保持原适用窗口；明确修改有效期使用 Fact 操作。实体合并和拆分按同一窗口规则迁移归属。

## Namespace 配置与配额

Namespace 模型选择写入 `models` 配置的 Namespace 覆盖。TOML 保存期望值，配置服务验证并发布有效快照。

Namespace 的 `enabled_features` 指定启用的认知能力。`formation` 保存原生形成开关、默认值、词汇及数量限制和实际字符切分限制。

`namespace.quota` 默认无上限，写入需要实例管理权限。请求并发额度由同一 Namespace 的实际 HTTP、SDK、CLI、MCP 和 RPC 请求共享。存储额度在 Authority 提交时检查，计量 Namespace 的原件、Observation、片段、认知修订、外部资源和主体身份的逻辑记录，以及按内容哈希去重的原件字节。使用量同时返回各项组成。共享派生表示、模型调用事实、操作收据、Job、导入工作集、索引和 SQLite 开销单独列为额度外空间；超额用户数据提交回滚，已保存的模型结果和操作历史保留。

## 数据移动

Namespace Bundle 携带逻辑对象、精确来源、修订、选定内容及向量。安装保留认知与材料身份，并建立安装记录。在线备份从 SQLite 一致快照枚举其引用的不可变内容，恢复到新实例。

跨环境移动已有实例数据使用实际授权的副本或导出包；`temp/` 内容不会随 Git 自动转移。原实例、原件和付费调用事实保留，已知历史数据库版本的转换按实际数据完成。公共产品合同在真实消费者边界承接，内部结构按当前 owner 完整迁移。

完整产品范围与当前实施依赖见[开发目标与计划](../../GOAL.md)。
