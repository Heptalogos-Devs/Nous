# Rust Native 计算接口与生命周期

## 工作区

`native/vcp` 保存来源可追溯的纯计算；`native/retrieval` 组织 Tantivy、USearch、查询通道和 Nous 数值扩展；`native/python` 提供薄 PyO3 绑定。

Python 使用包 `nous_native`，maturin 构建与 CPython 3.14/NumPy 兼容的发行物。Rust 采用可支持现有依赖的稳定工具链，沿用可用锁定依赖并更新必要版本；不依赖 nightly，也不声称 Rust/Python 有项目自行发明的 LTS。

## 粗粒度接口

```text
NativeEngine(settings: EngineSettings)
  build(assets: FrozenAssets, options: BuildOptions, output_dir, base?, cancel) -> BuiltSnapshot
  open(path, expected_identity) -> NativeSnapshot
  close()

NativeSnapshot
  identity, capabilities, statistics
  observe(signals: QuerySignals, options: ObservationOptions, cancel) -> QueryObservation
  execute_program(program, signals, allowed_rows, options, cancel) -> NativeQueryResult
  think(observation, stages: ThinkingStages, options, cancel) -> ThinkingResult
  neighborhood(node_rows, options, cancel) -> NeighborhoodResult
```

程序执行器按已编译的算子图取得观测、候选、校准和排名，在同一快照借用中复用阶段结果。Thinking 使用实际观测；高级调用可单独取得观测和路线。程序与端口见 [Wave 程序](program.md)。

函数返回真实结果或类型化错误，不返回带 success=true 的空实现。lexical 不需要模型或向量；dense 需要对应向量；VCP 需要几何/概念/图能力；空种子是算法允许的输入，不当作异常，但 capability 缺失不能伪装成已经运行 VCP。

## 输入形状

FrozenAssets 见资产专题。QuerySignals 包含 raw_query[D]、effective_query[D] 或 context 矩阵、core_rows[K]、ghost_vectors[G,D] 与 ghost 描述、实际 query_text。大数组分别传入，metadata 不嵌入 JSON 向量。纯词项程序的向量字段可以缺省；需要向量的程序使用与空间一致的实际向量。

CandidateSet 保留 candidate row、各通道得分与名次；CalibratedPool 保留该候选总体、观测和校准参照。ThinkingStages 保存 stage name、candidate_rows 和 top_k。所有行号属于 snapshot；QueryObservation 同时绑定 snapshot/space/算法配置，跨快照使用报 `wave.observation_mismatch`。

Python 侧只提供少量输入值类，实际 C-contiguous NumPy 数组通过 rust-numpy 传递。无需一个拥有几十个可选数组参数的 build 函数；可以用四个职责明确的 bundle：Candidates、Concepts、SourceFacts、Associations。bundle 只是数据值，无服务依赖。

## 内存所有权

构建时将外部可变数组一次转换成 Native 自有连续 Vec/Arc 或本轮确实可用的只读映射；不在 detach 后借用调用者仍可写的 NumPy buffer。read-only 标记本身不能约束其它别名。查询小向量可以复制；大常驻矩阵复用 snapshot，不每次重新传输。

返回行号/分数通过 owned NumPy arrays 或只读受 owner 生命周期保护的视图交付。不得标榜零拷贝却在每个阶段转成 Vec<Vec<f32>>/JSON；也不为省一份构建期拷贝引入共享可变内存风险。

## 并行与取消

Rust 计算通过 Python::detach 运行，跨语言只在操作边界。Engine 有一个配置有界的 Rayon 池，所有建图与查询使用它；热循环不调用 Python。USearch/Tantivy 内部线程按相同资源目标配置，避免每个查询各开满线程。

Python WaveOperations 经现有 Execution.blocking 调用并传 CancellationToken；取消后由该公共能力等待实际工作退出。Token 是 Rust AtomicBool；图迭代、几何批次、候选扫描与索引阶段检查。取消后等待计算退出，再释放 snapshot 和临时目录。长第三方不可中断调用完成前资源仍被持有。

## 错误与配置

NativeError 具有 code、message 和小型 details，经统一映射成为 NousError。参数非法、space/shape 不匹配、索引损坏、能力缺失、取消分别表达。不要靠搜索任意错误文本中的关键词分类。

算法配置由 Rust serde/schemars 类型维护。默认值/schema 在构建时导出为包内资源，Python Configuration 使用这些纯资源定义查询目录和校验；数值执行按同一类型解析。同次查询共享向量池、范数缓存与有界 workspace；重复的纯数值 helper 仅在数学及浮点语义相同时合并，不以“统一”之名替换有不同含义的残差、概率或质量量。不要保持一套 Python 手写默认值和另一套 Rust 默认值。

## 索引后端

Tantivy 与 Jieba 承担词项/中文分词；USearch 承担候选与 Concept 双向量索引。成熟依赖已能实现的索引、序列化和矩阵计算不自行重写。Native 只能在 Python 提供的组件目录管理派生资产，不知道业务数据库地址和 CAS 根。

Tantivy 为正文和标题建立独立字段，标题仅由文稿级候选承载。查询使用同一分析器的 SnippetGenerator 返回正文片段在检索文本中的字节范围，并标明标题和正文的匹配字段；原文坐标映射由内容 owner 完成。

## 真实核心和完整接缝

具体接口、候选规划、空图、长序列、Thinking 记录及构建/open 的分工见 [检索核心](engine.md)。这些行为在 retrieval crate 实现，PyO3 不持另一套 query/engine 业务。

build 显式关闭构建期 writer 并释放 reader 的映射，最后原子写入完成清单并返回 BuiltSnapshot；Wave 打开同一稳定目录校验。readout 接收程序传入的候选值，不自行再召回。已有向量的 Python 完整查询使用带空间身份的 VectorRef。

冻结快照打开已提交的 Tantivy reader，不创建索引或取得 writer 锁；构建和增量更新只在各自的 staging 目录取得 writer。多个读者可以同时打开并查询同一已发布组件。

核心的 Engine 持有有界 Rayon 池、关闭状态与实际工作锁；Snapshot/冻结资产由 retrieval 持有。PyO3 的 Engine/Snapshot 只借用核心对象并 detach。打开可传取消令牌，完整性摘要读取按块检查取消；Wave 对读者提供范围化借用并持有有界缓存。

CandidateSet 保存快照身份、有序候选与各 lane 的原始分/名次。显式校准产生 CalibratedPool，并保存授权总体产生的 BM25 参照和联合选择；RiverMemo 消费该池，不再调用 select_superset 或索引。


## 增量构建与旧代借用

同集合的前一正式快照在整个派生构建期间持有生命周期借用；只有空间、构建配置及选项一致时才能作为基代。新目录复制独立词法文件，不对旧代文件写入或使用硬链接。按稳定候选键比较正文、标题和文档组键，只提交新增、替换及删除；快照中的稠密行号变化不触发无关文稿重写。Tantivy 默认 BM25 统计包含尚未合并的删除项，因此快照另存活跃词数和词频；增量更新只分析新旧变化文本，查询使用活跃统计，结果与干净构建一致。删除记录同时超过活跃项和 512 项、或段数超过 32 时压实，限制累积垃圾。

每代保存并校验候选键和文档组键到内部数值 ID 的映射；存续对象沿用基代 ID，新对象使用不回退的计数器，查询再映射到当前快照的稠密行号。向量代从校验过的旧代私有加载并更新；旧代仍可搜索。若删除垃圾将超过阈值，先决定重建受影响的向量通道；重新打开时校验映射、数量和全部活动向量。HNSW 的近似拓扑可能因增量顺序与全量重建不同。概念、事实、关联及构建选项均未变时复用图资产，否则重新计算。新代独立写清单并最后发布；失败或取消不改变旧代，未完成组件按正常清理规则回收。
