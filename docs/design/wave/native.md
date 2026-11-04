# Rust Native 计算接口与生命周期

## 工作区

迁入当前 `native/vcp`、`native/retrieval`、`native/python`。VCP crate 保存来源可追溯的纯计算；retrieval 组织 Tantivy、USearch、查询通道和 Nous 数值扩展；python 是薄 PyO3 绑定。现有 legacy 解耦成果有用就迁移，删除被取代副本和专属构建支撑，不恢复旧业务层。

Python 使用包 `nous_native`，maturin 构建与 CPython 3.14/NumPy 兼容的发行物。Rust 采用可支持现有依赖的稳定工具链，沿用可用锁定依赖并更新必要版本；不依赖 nightly，也不声称 Rust/Python 有项目自行发明的 LTS。

## 粗粒度接口

```text
NativeEngine(settings: EngineSettings)
  build(assets: FrozenAssets, options: BuildOptions, output_dir, cancel) -> BuiltSnapshot
  open(path, expected_identity) -> NativeSnapshot
  close()

NativeSnapshot
  identity, capabilities, statistics
  observe(signals: QuerySignals, options: ObservationOptions, cancel) -> QueryObservation
  recall(observation, options, allowed_rows, cancel) -> CandidatePool
  search(query: NativeQuery, options: QueryOptions, cancel) -> NativeQueryResult
  readout(observation, candidates: CandidatePool, options, cancel) -> NativeQueryResult
  think(observation, stages: ThinkingStages, options, cancel) -> ThinkingResult
  neighborhood(node_rows, options, cancel) -> NeighborhoodResult
```

search 是一般消费者入口，内部根据 mode 组织 observe/candidate/readout，最多计算一次同次观测；需要同时 Thinking 时同一 search 请求带 stages，并在返回前复用该 observation。高级进程内调用可以单独 observe/readout/think，它们消费同一个类型化观测，不重新构建图。

函数返回真实结果或类型化错误，不返回带 success=true 的空实现。lexical 不需要模型或向量；dense 需要对应向量；VCP 需要几何/概念/图能力；空种子是算法允许的输入，不当作异常，但 capability 缺失不能伪装成已经运行 VCP。

## 输入形状

FrozenAssets 见资产专题。QuerySignals 包含 raw_query[D]、effective_query[D] 或 context 矩阵、core_rows[K]、ghost_vectors[G,D] 与 ghost 描述、实际 query_text。大数组分别传入，metadata 不嵌入 JSON 向量。纯 lexical 查询的向量字段可以缺省；需要向量的模式必须提供与空间一致的实际向量，不能以零向量补齐。

CandidatePool 保留 candidate row、各通道得分/名次/来源位。ThinkingStages 保存 stage name、candidate_rows 和 top_k，不使用物理日记目录。所有行号属于 snapshot；QueryObservation 同时绑定 snapshot/space/算法配置，跨快照使用报 `wave.observation_mismatch`。

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

Tantivy 与 Jieba 承担词项/中文分词；USearch 承担候选与 Concept 双向量索引。成熟依赖已能实现的索引、序列化和矩阵计算不自行重写。Native 只能在 Python 提供的 index/staging 目录管理派生资产，不知道业务数据库地址和 CAS 根。

## 真实核心和完整接缝

具体接口、CandidatePool、统一候选规划、空图、长序列、Thinking 记录及构建/open 的分工见 [检索核心](engine.md)。这些行为在 retrieval crate 实现，PyO3 不持另一套 query/engine 业务。

build 返回已经关闭 staged readers/writers 的 BuiltSnapshot；发布后 open 一次。readout 必须接收外部 CandidatePool，不自行再召回。think 接收显式 options，不用绑定层默认值覆盖请求。已有向量的 Python 完整查询使用带空间身份的 VectorRef。

核心的 Engine 持有有界 Rayon 池、关闭状态与实际工作锁；Snapshot/冻结资产由 retrieval 持有。PyO3 的 Engine/Snapshot 只借用核心对象并 detach。打开可传取消令牌，完整性摘要读取按块检查取消；发布后的快照只打开一次。

CandidatePool 保存快照身份、有序候选与各 lane 的原始分/名次，以及完整授权召回池产生的 BM25 参照、联合选择校准。普通 recall 完成一次校准和并集选择；固定外部池使用相同校准公式但保留明确给出的集合。readout 消费已准备的池，不再调用 select_superset 或索引。
