# 当前知识视图与检索发布

## 用户保证

保存返回的 revision 立即可读。`query` 缺省查询执行期间捕获的一次已提交当前视图，至少包含调用开始前已经完成的保存；并发新写入不改变这次已经捕获的视图。结果带 view_generation、snapshot_ref 和精确 revision refs。

current 选择对象的最新采用版本。实际事件是否在今天有效由 valid 条件单独指定；历史经历仍可作为当前保存的知识被找回。旧快照查询固定其旧版本和计算资产。

## 两种正式状态

Namespace 保存 `content_generation`，所有实际改变当前可召回集合或其证据视图的事务只推进一次：纳入召回的文稿新建/修订、归档/恢复/召回设置、活动标注及绑定变化、显式基础依赖更正。普通评价、未采用提案、模型试运行、日志，以及未进入召回且未改变任何明确依据的碎片笔记，不推进。单独修订 Concept 的未绑定意义版本也不重建现有图。

Wave 已发布快照保存 covered_generation、profile_signature、space_signature、精确正文/标注/概念/支持组清单。它是投影状态；内容 generation 永远由认知事务拥有。

不把 dirty 布尔值、事件表、任务队列和 generation 同时维护成四份状态。持久未完成状态由“内容代次大于相应 profile 的已发布代次”直接表达。

## 投影视图与查询程序

视图按所选程序需要的资产与实例空间配置构建。词项程序不要求向量空间；需要向量的程序使用 `cognition.search.space`。默认程序及能力不足时使用的程序由 Wave 配置确定，执行结果记录实际程序。

不同空间或证据策略使用不同 profile key。key 包含空间语义、`include_review`、`include_exploration`、`include_work`、材料与组合同、候选布局及构建配置；改变实际资产要求新投影，单次查询参数不要求重建。模型请求失败时返回实际错误。

## 捕获与构建算法

1. 根操作固定配置和 Namespace。读取该 profile 已发布快照及 namespace generation，完全匹配时可直接持有 immutable snapshot，热查询不先捕获整库正文/标注。
2. 需要更新时，取得同 Namespace/profile 的构建协调锁，再检查一次。锁只协调该项构建，不阻塞文稿写入，不是实例级请求锁。
3. 在一个短 ReadTx 中由各材料 owner 捕获 generation G、当前材料事实和精确内容引用，并按共同资格事实判断组资格。以材料和组合同、精确修订、正文、来源、活动标注及候选布局匹配可复用投影单元；变化材料展开必要来源，未变组按文档键共享祖先节点。正文读取和解析在事务外进行。标注 owner 返回多个精确选区，不把 occurrence 包成跨越无关正文的 min/max 范围。
4. 网络向量、CAS 读取、Native 构建全在事务外。同一正文和表示在一次构建中只准备一次，复用已采用精确向量，构建完整不可变资产；不要用旧图加上新正文后过滤凑当前视图。
5. 在稳定组件目录完成构建并校验完成清单。短 WriteTx 登记 WaveSnapshot。仅当 namespace generation 仍是 G、profile 仍相同时更新该 profile 的 current 指针；否则保留此快照供本次已捕获视图使用，不回滚内容、不追赶循环。
6. 本次查询使用捕获的 G；后来写入由下一次 current 查询或显式 refresh 处理。返回 captured_generation，必要时提示已有更晚版本。不要让持续写入造成无限追赶或持有数据库事务等待模型。

取消只影响本次未发布工作，已保存文稿不受影响；同 profile 等待者可在后续接管构建。直接读取正文、精确旧快照及其它 profile 不等待本次构建。

## 类型化 Wave 合同

```text
CorpusView(namespace_id, generation, profile,
           documents[exact refs], annotations, concept_revisions,
           source_sequences, support_groups, candidate_specs)
async with Wave.build_product(view, resolved_profile) as product:
    Wave.publish_in(tx, product) -> SnapshotDescriptor
    Wave.promote_in(tx, snapshot_ref) -> None
WaveOperations.query(WaveQuery) -> WaveSearchResult
```

清单路径入口转换为同样的 view，再进入共同 build。认知代码不导出临时 JSON，也不访问 Wave SQL/Native 内部行号。Wave 只认识可检索正文及计算引用，不理解 Journal 的采用规则。

`CorpusCapture.directory_in` 读取轻量目录，`expand_in` 在同一读事务中展开变化材料；`ProjectionBuilder` 声明 DocumentCandidates、标题、独立 occurrence 及路径。Wave 负责准备单元与数值组件；认知用例在自己的 WriteTx 检查 generation、发布结果并在条件成立时提升当前指针。

## 候选和证据

Journal、remember=true 的 Note，以及明确开启召回的 Work 提供当前修订。归档文稿不再贡献自身候选或概念路径；其它 active 文稿明确引用它的历史内容仍可保留来源关系。archive 是收纳选择，不声明那段经历错误。

认知投影声明 DocumentCandidates，由 Wave 使用共同内容批量准备，在同一 TextView 上生成整文和有界局部候选。whole/section/block 必须指同一 exact revision；召回前就使用文稿分组，不采用 top_k 乘常数后去重。具体行为见 [准备](../wave/corpus-preparation.md)和[分组](../wave/grouped-delivery.md)。局部候选只拥有实际覆盖的 occurrence 和路径片段，截去中间 occurrence 时保留断点，不补新相邻关系。

概念节点按 exact 意义版本映射；未标注正文仍进入 lexical/dense。被更正旧 revision 的 paths/几何贡献从当前投影移除；历史查询只使用其精确旧快照。

正式派生 Journal 的解释性结构按 [结构贡献](structure-contributions.md)进入同一投影；独立来源与支持组沿现行 evidence-projection 计算，不把 source link、一次模型输出和同源反复摘要当成独立支持。current_basis 待复核状态按 temporal-use 定义，在捕获时计算。

## 失败与使用状态

保存和刷新分开：未请求刷新时返回 not_requested；用户要求刷新而失败时返回原 revision 与失败原因。正文保持可读，索引新鲜度由捕获代次和当前代次表达。

`query` 在无法建立所请求当前视图时返回 projection failure；锚定的查询使用指定快照。

未请求保存的查询只返回内存交付。保存或阅读整理时，精确证据成为正式成果；调用者也可随后采用已完成的内存交付。反馈通过该成果及所属 E 键固定实际目标，见[查询情境](query-use-context.md)。普通保存不为回读原文保留计算快照；明确锚定的保存和可用的反馈快照按历史计算需要保留。快照回收后，已保存 E 与当时的排名依据仍可从证据读取，反馈标记快照不可用。纯 `@ref` 直接读取正式材料，不构建检索快照。快照所属范围使用 typed owner，集合名不承担身份协议。

## 认知与内容血缘的接缝

记录原始经历的 Note/Journal 有稳定 source-origin 身份，各正文修订共用这项来源身份，避免编辑次数变成证据数。派生 Journal 使用明确绑定材料的来源组合；revision_base 和启发上下文仍可追溯，但不会产生新的独立事实见证。

capture 在同一读事务固定 exact revision、source bindings 与原始首版 ArtifactRef。初始无来源文稿以稳定 record 身份作为 source-origin，各次修订只更新当前正文/图，不产生新的独立根。多层派生沿 reference/current_basis 的正式 revision DAG 归并这些根；同源的多个历史版本仍归一项来源。外部 Artifact 使用 ResultStore.support_roots；inspiration-only 派生的来源根为空，正文与原启发引用仍可读取和检索。

CorpusView.evidence 明确交付根身份、根 ArtifactRefs、每份文稿的根集合及可贡献原始序列的文稿；构建不再从生成血缘重解释这些正式认知关系。输入与实际构建配置一起固定在快照中，current 查询按捕获的知识代次和实际视图策略选用相应视图。

ResultStore.provenance 返回真实生成血缘；cognition 的支持投影解释关系依据，两者的职责分别保留。只有实际被选择的当前正文/标注贡献自己的图结构，历史 source refs 不能让退役正文的整套旧路径重新混入当前图。

Namespace.content_generation 表示可召回知识/证据视图的变化序，不是所有表的全局事务日志。各 owner 在提交时明确是否 affects_retrieval；普通不参与召回的笔记保存只返回 not_in_scope，不能每记一句草稿就触发全库建图。
