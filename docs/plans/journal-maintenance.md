# Journal 可控维护、程序使用与共同能力整改

## 本轮目标

基线为 `c2944b1675ddb79b9de17beb4f9522349fbbc8ce`。完成已有 Journal 的精确修改、独立语言写作意图、来源继承、反馈处理及进程内使用；补齐 Prompt 编辑上下文、检索投影复用、文稿分组、部分失败诊断和认知 CLI 接入。

完成后的用户流程为：记录/形成新 Journal → 查阅正文与媒体来源 → 给出意见或明确修改 → 取得局部修订或显式重写 → 采用并读差异 → 当前检索更新 → 原版本与原引用可追溯。宿主程序直接使用同一组操作，CLI 只是入口。

范围不含 DBOS 上线、后台守护进程、HTTP/MCP、远程 SDK、WorkContext/Steward/Episode 实现或反馈自动学习。其有效设计与接入次序记录在 [后续路线](cognitive-use-roadmap.md)，不以空接口宣称完成。

## 必读合同与真实负责者

先读现行 AGENTS（本包不修改）、docs/ARCHITECTURE、src/nous/INDEX，再读本计划和 [维护架构](../design/cognition/maintenance-architecture.md)。每个任务只继续读取自己消费的专题与真实调用方。

本轮依赖已有 Python 3.14、Pydantic、SQLAlchemy/SQLite、MarkdownIt、Jinja、CAS、Execution、OpenTelemetry/structlog、Tantivy、USearch、PyO3/Rayon。优先使用现成正式接口；不重写客户端、解析器、线程调度或数据库机制。当前内部 API、表形状、目录可以直接更改，迁移所有实际消费者后删除旧路径。

本包不要求评审者运行本地测试或 CI。Coding Agent 完成实际功能并按用户入口定位问题；用户独立验证。验证围绕完整通路，不引入 TDD、逐项测试/提交/评审或专用测试平台。

## 工作依赖

```text
共同内容/错误结果 + 类型合同
   ├─ 文稿保存核心与进程内操作 → 精确编辑 → 独立语言任务/草稿 → 反馈处理
   ├─ 批量标注读取 + 正文准备 → 认知投影 → Native 分组 → 类型化 QueryResult
   └─ Prompt 上下文 + CLI/l10n/实例选择
全部接回真实操作 → 文档原地更新 → 交付实际使用结果
```

可以并行处理独立的 content/Native/Prompt 责任。引用类型、文稿表和共同错误状态由一个负责人统筹，避免同时维护多套值形状。每个任务的完成意味着进入真实调用链。

## 文稿核心与公开操作归位

**修改位置**：原 cognition/records.py、record_reading.py、journalize.py、retrieval.py，及 runtime/bootstrap.py、cognition/contracts.py/schema.py、所有 CLI 和相关消费者。

**实施**：

1. 按 [维护架构](../design/cognition/maintenance-architecture.md) 将写作、读取、草稿收为 writing/，检索捕获/投影/操作收为 retrieval/；迁移实际符号和 import。旧文件移除，不留 re-export 兼容。
2. WrittenRecords 只拥有正文身份、事务内采用、成员状态、来源和 generation。移除 annotations/retrieval 依赖与 complete_save。RecordReader 提供精确/批量读取，不因继承层而绕回应用操作。
3. 在 writing/operations.py 实现 RecordOperations，拥有真实 root 操作、准备字节、事务、反馈/草稿同事务关联和 AfterWrite。用共同 `finish_write` 过程组织请求的标注/刷新；正常 defer 成功，明确阶段失败 partial。
4. `await runtime.cognition(namespace)` 返回绑定稳定域身份的实际操作集合。连接、CAS、执行、配置和 Native 仍共享 Runtime 中已有实例；不同域句柄不更改全局选择。
5. 新返回值固定为 RecordOutcome.record（CommittedRecord），并分开 committed_generation 与后续 annotation/retrieval 的实际 generation；不能为了展示最新值篡改“本次正文在哪代提交”的信息。
6. 所有手工 create/edit/replace、草稿采用、反馈更正、归档/恢复都进入新的统一操作。CLI 不调用 `*_in` 或拼事务。

**产物**：没有模型也可在同一长 Runtime 中记录、读取、手工修改、归档和历史回读；一次逻辑调用有一个根 operation，后处理结果可独立解释。

## 精确编辑与来源继承

**修改/新增**：content/edits.py、content/citations.py、writing/records.py、writing/operations.py、writing/contracts.py、models/input.py 与 InputSpec。

**实施**：

1. 实现 [精确编辑](../design/cognition/exact-editing.md) 的变更值、解析器和逆序应用算法。块/选区由现有 TextView 解析；引文只在范围内做精确唯一匹配。
2. RecordOperations.apply 使用 exact base；校验后发布全文，在短事务用原基准提交。replace 走相同核心；diff 使用标准库展示真实两版差异。
3. 建立 CitationContext：保留旧 source keys、固定新来源、解析成功路线的实际键。InputSpec 支持显式局部根键，所有路线和媒体分片通过共同 planner 保留它。禁止在 writing 事后按字符串猜 split 后来源。
4. 引用解析移到共同 citations 能力，records 不再自行 MarkdownIt.parse 一遍实现另一个链接合同。未改动正文及链接保持原字节，明确移除的绑定经同一校验。
5. correction 继续调用 basis 的实际失效语义；edit 不凭意见词汇触发更正。多人/并发基准变化返回冲突并保留编辑稿。

**产物**：调用者能精确改句、段前后插入、删除或全篇替换；未涉及内容及来源保持；旧版本可读；不是让 LLM 再摘要一遍。

## 创建、编辑建议、重写、反思任务

**修改/新增**：writing/generation.py、drafts.py、tasks.py、drafts 表、models/tasks.py、Prompt 资源与 cognition/configuration.py。

**实施**：

1. 移除 journalize 的 target_revision 和 revision_base 更新路径。共同来源解析识别正式 Journal 身份；旧 Journal 主输入提示使用 rewrite/reflect。
2. 保留现行 journal-draft 的用户规则与 no_material 解释，只用于新篇。新增 journal-edit、journal-rewrite、journal-reflect 三个专一 TaskDefinition 与 Prompt。模型任务只产生文字/小型 edits；无额外审核模型。
3. edit 输出通过共同文本能力解释为 edits＋完整正文＋diff；模型提供的 T 键/引文由程序定位。rewrite 输出完整正文；reflect 采用新 Journal，旧文稿不变。
4. DraftStore 统一保存四种成功产物、冻结 base/sources/意图、反馈引用和 adopted_revision。ProducedTask 及草稿描述同事务采用；正式文稿采用另一个事务。保存失败沿现有恢复信息处理，不重新发模型请求。
5. RecordOperations.adopt 在同一事务完成正文、draft adoption 和 feedback resolution。重复采用返回原结果，冲突不自动切到 latest。编辑器改写整个草稿需明确变更为全文输入，保留生成与人工修改来源。
6. 模型资源/Prompt/输出合同进入现有配置与 describe。缺模型时人工操作继续可用，语言操作报实际缺项。

**产物**：新篇形成与 Journal 更新职责分离；使用者能明确选择局部改动、重写或另写反思，重复调用不会隐式反复压缩旧 Journal。

## 反馈与正式查询情境

**修改**：cognition/feedback.py、feedback_schema.py、retrieval/contracts.py/operations.py、wave/contracts.py/operations.py/snapshots.py、CLI feedback/retrieval。

**实施**：

1. 定义 QueryContext/QueryResult 与文稿分组返回。context 由实际 query prepare 产生，固定实际参与片段、过滤、空间/profile、快照、模式与交付选区。
2. CorpusView/snapshot descriptor 增加类型化 owner；Namespace 校验用字段而非 collection 字符串前缀。metadata/owner 读取不打开 Native 索引。
3. FeedbackOperations.record_query 接收正式 QueryResult；CLI 只反序列化。普通反馈固定 exact target，默认不持久保存每次完整 query。
4. 实现反馈 read/list filters/resolve 与小型处理记录。明确 edits 通过 RecordOperations；反馈 owner 仅提供同事务解决关联，不再另写 correct 的正文提交。
5. natural-language feedback 仅在显式调用 propose_edit 时作为意见输入；记录和已应用状态分别展示。

**产物**：评论可检索、可处理、能追溯其目标与语境；没有全局降权或自动改认识。

## Prompt 维护上下文

**修改**：models/prompts.py、TaskDefinition、包内 prompt.toml、CLI tools prompt 与配置示例。

**实施**：

1. PromptManifest 增加 documentation 与每条 message 的 description，仅供人读取。
2. describe 从真实任务输入/变量/Schema和有效模型配置生成 PromptContext。保留当前静态 system/动态 user 规则，不复制 Schema 常量。
3. edit 目录生成 CONTEXT.md；show/diff/restore 明确包内与实例资源。import/validate 使用同一加载与校验入口。
4. 将执行指纹与说明指纹分离；真实模板或输出规则改变影响执行指纹，人类注释改变不影响模型请求。当前 frozen resource 和 fallback 继续使用一次捕获。
5. 新 Prompt 示例是起点；保留用户既有写作规范、实例覆盖与正反例，不整体替换已有 system/user 文件。

**产物**：人打开任一 Prompt 就能知道用途、输入、变量、Schema、模型能力、角色及阶段；修改仍是普通 Markdown/Jinja/TOML。

## 多媒体阅读与投影准备收拢

**修改**：content/artifacts.py/results.py/text.py/citations.py/projections.py；新增 content/preparation.py 或按现有模块职责实现；media/preparation.py；cognition 投影；wave/assets.py。

**实施**：

1. 实现 [共同阅读与准备](../design/content/reading-and-preparation.md)。批量取得 Artifact/blob 元数据，文本与非文本来源使用同一 ContentReadView。
2. 为 TextView 提供完整块分片、精确切片及缓存纯结构。词项构建不逐候选跑媒体 inspect，不对每个片段重复读全篇/parse。
3. CorpusView 候选声明支持 DocumentCandidates；认知投影只交正文与布局，Wave 在一项准备中展开 whole/块候选。实验 ExactCandidate 同样归一后进入本引擎。
4. 采用 PreparedText 做引用 label 投影、语义向量准备和来源映射。已有确定性投影批量复用；只有实际需要持久输入表示时才由内容/向量 owner 采用。
5. AnnotationOperations 增加 ReadTx 内批量读取，投影不再重组标注表，保留 occurrence 多选区。
6. 热查询先比较代次，不先 capture 全库；冷构建保留 G 的一致视图，向量与 Native 仍在事务外，发布只打开一次。
7. 补齐派生 Journal 的 interpretation 结构输入，按现行支持组公式去重；默认增益不偷偷修改。正文、候选和独立来源身份分开。

**产物**：保存后首次检索减少重复准备，来源可以跨媒体读回，原始/派生图贡献正确；完整 VCP 数学仍在 Native。

## 分组召回与 Native 交付

**修改**：native/retrieval 的 bundles/snapshot/query/lexical/vector，新增内聚 grouping.rs；native/python 转换与 type stubs；wave/query/contracts；cognition query。

**实施**：

1. 构建保存 group_row；group 属于精确文稿，切片不额外占一个“文稿名额”。
2. 按 [分组合同](../design/wave/grouped-delivery.md) 在每路召回、合并和最终读出使用组政策。Tantivy collector 在扫描评分时聚合；USearch 分批过滤已饱和组，所有有界扫描在 Rust 内完成。
3. 按组整理过的候选池作为固定池进入 VCP，防止 prepare_pool 再次按片段截断。保持同次观测、质量分/排名键和原有算法参数，返回有效单位和覆盖。
4. 删除认知层 top_k*4、再去重和补查；默认结果为文稿组与其最佳片段。外部 CandidatePool 不能被引擎静默补候选。

**产物**：长文不再靠大量片段吃掉短记录名额，top-k 的产品含义和工作量上限清楚。六种模式及高级接口共用一套分组。

## 部分完成、单根操作与本地化

**修改**：common/operations.py、observability/instrumentation.py/diagnostics.py、runtime/bootstrap.py、presentation、所有新认知 CLI。

**实施**：

1. 实现 [操作结果](../design/foundation/operation-outcomes.md) 的报告与终态；Record/Media/Writing 用例使用同一状态和值。已恢复的 attempt 不误记 root partial。
2. runtime logs 和 recent_problems 从根终态读取，partial 保留实际错误、stage、已完成 refs；runtime check 使用该查询，不维护另一份状态。
3. annotate CLI 调完整 annotate，移除自身 propose/adopt 编排。人工 proposal 和 draft-only 都用正式参数。
4. 所有认知命令迁到 presentation 翻译入口。无 H1/无标题持久值为 None，界面本地化默认名称，JSON 保留真实值。正文/作者标题不翻译。
5. InstanceLocator 接入可读 profile 和当前选择，默认解析只在一处。现有 NOUS_INSTANCE、显式路径继续是正式选择来源；不要求用户输入 UUID。
6. runtime.check 增加准确的近期 partial 摘要与正文准备阶段信息，不能通过自己跑模型或重建索引来检查状态。

**产物**：错过一次 partial 响应后仍能从操作日志找回原因，en 输出无中文默认标签；程序和 CLI 同一条完整关联。

## 数据和文档迁移

所需变化包括成功写作草稿的 intent/payload、反馈处理关联、snapshot owner/group rows、标题可空以及纯解析缓存。当前内部数据可按授权开发实例直接重建；不要为旧内部 DTO/serializer 新增版本兼容。不得默默删除当前用户实例，先按实际授权明确选定目标实例；未授权的数据路径不动。

业务 Journal 的历史版本继续按新实现保留，不把“内部可重构”误用为删除新产品的历史能力。

用交付包根目录 APPLY.md 所述方法并入文档（该交付文件不留在项目 docs 里）；AGENTS 本轮不修改。实现后更新受影响专题、源码 INDEX 和 CURRENT 中实际能力，不枚举用户同步的原始讨论，不宣称未实现的 DBOS/WorkContext 已完成。

## 最终交付

完成 [实际使用流程](../development/journal-maintenance-use.md) 中的程序内与 CLI 操作。用户给出的 501 文稿、长文＋短文和 partial 情境直接复用，解释实际改善与剩余原因。临时数据/探针按现有 temp 规则，不提交一次性脚手架。

交付说明包括实际产品能力、修改的职责、准确命令/程序调用、数据需要重建时的具体操作与真实未完成项。没有普遍测试数量、通过率或 CI 门禁目标。

## 实施时必须一同迁移的接缝

来源槽与 SourceBinding 多选区按 exact-editing 的完整字段迁移，覆盖 ResultStore.actual_inputs、媒体 planner、TaskDefinition、草稿、文稿引用与 Wave projection；不保留旧单选区分支。新写作任务的未变化语义分别处理：edit 空 edits、rewrite 同文 unchanged、new/reflection 精确 no_material。DraftOverride 不丢原生成结果，已采用后不同覆盖正文报冲突。

所有显示 title 的消费者按 nullable 数据合同迁移；新认知命令的保存、历史、标注、反馈、差异与 refresh 全部使用 presentation。partial 已保存结果的 CLI 退出码统一为 3，并保留 stdout 结果。

文稿原子提交遇到未知程序错误照常抛出，只有已知的提交后阶段失败才转 partial。更新共同诊断 reader 与 runtime check，而非仅增加新的日志字段。
