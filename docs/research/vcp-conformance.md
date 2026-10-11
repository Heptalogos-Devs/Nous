# VCP source conformance

[返回研究入口](README.md)

## 来源与状态

本轮冻结 `lioensky/VCPToolBox@e03b891d42055cdf3cab5dc961cc71dd5facd65a`。精确文件 blob 与研究基线由 [source manifest](corpus/cognitive-retrieval-sources.json) 拥有。本页描述已读 production Rust 合同及当前差异；独立 reference kernel 已开始实现，VCP adapter 已进入普通 Kernel query 路线；完整 fixture matrix、外部 benchmark 与算法消融仍未完成，不能将源码研究写成 `reference-parity`。

当前 CI 只使用 compact discriminating goldens。以下 source research 描述包括完整原生输出的生成历史；完整矩阵已经移到 ignored `data/research/vcp-reference/e03b891d42055cdf3cab5dc961cc71dd5facd65a/full-source-fixtures/`，不作为当前 CI case 数或覆盖声明。2026-10-06 重新核对 native checkout HEAD 与 manifest，均为本页冻结 commit。Reference algorithms、adapters 与 Serving readout 保留。

当前 tracked fixtures 约 425 KB，每个 primitive 保留 nominal、zero/empty/boundary 和能区分公式的 adversarial case。Query morphology 保留 atomic/merge/empty/deep narrative/Omega override，scoring 保留 atomic/propositional/narrative/empty；Sense/graph 保留合流 state，anchor 保留 exact/fallback，DTSC 保留 normal/retention-trust/guard；EPA training 的不同密度 weights 对 basis 与 energies 作 golden 对照，能区分 `sqrt(weight) * centered_vector` 与只加权 mean。Full pipeline 保留普通组合和 state-budget 组合，full readout 保留正常与 empty-fields 两例。`cargo test -p nous-retrieval --test vcp_reference` 的 23 个测试通过；它验证这些保留案例，不声明 full matrix parity。


已完整研究 `memo_pipeline.rs`、`memo_sensing.rs`、`memo_artifact_builder.rs`、`memo_dtsc.rs`、`rivermemo_topology_v3.rs`、`TagMemoEngine.js`、`rag_params.json`、要求的 Deep Dive/Topology V3 文档及 unified geometry probe。已进一步读取 lib.rs 的 EPA/Intrinsic Residual owner、TagMemoV10 配置快照、RiverMemo 的 native 参数映射和 RAGDiaryPlugin 的 TimeDecay 调用与实现。参数须以 frozen effective config 为准：Rust fallback defaults 与 rag_params 的 alpha、support count、contact thresholds、language penalty 有差异。

VCP 为 CC BY-NC-SA 4.0，Nous 为 MIT。本页只记录独立提取的数学合同；Nous 实现不包含第三方源码。Reference fixture 保存数值输入、输出、浮点容差与来源身份。

## 分项矩阵

`Label` 描述当前 Nous 实现与该项 reference 的关系；未完成的新 reference/adapter 明确标记待实现。参数相同不代表语义一致，排序相同也不能替代中间数值 parity。

| Component | Frozen VCP owner | Exact contract | Frozen Nous native | Reference kernel | Nous adapter | Label |
| --- | --- | --- | --- | --- | --- | --- |
| EPA | memo_pipeline analyze_epa；lib.rs EpaBasisTask | f32 query centering，basis dot 的平方能量概率，归一化熵、dominant axes 与 bridge resonance；query analysis 不读取 cached training energies | weighted representative PCA 与当前 EPA policy | query analysis 首个 pipeline fixture + 20-case EPA/field 边界矩阵；density sampler / weighted f32 basis builder 8 个 native case 通过 | 同一 VcpGeneration 的 stored producer/space 材料、f32 basis 与 immutable query analysis | uncertain |
| Residual | memo_pipeline analyze_pyramid / orthogonal_projection；lib.rs IntrinsicResidualTask | 每层 residual ANN，按返回序正交化 Tag，dependent axis contribution 为零；f32 投影累积，解释量归一到原 query energy | bounded residual search 与 Nous own residual contract | Pyramid 四个 actual ANN snapshot case；Intrinsic Residual 16 个 native task case 通过 | 同代 Tag cosine USearch；其返回序进入 Pyramid，无独立 embedding 调用 | nous-native |
| graph build | memo_artifact_builder build_fact_matrix / build_transport | 有序 file-tag 全 pair；位置/距离/semantic/reverse anchor 调制；log evidence、target inflow hub 校正与预算内 wormhole reserve | CognitiveRef/AssociationEvidence，support class quality 与独立 provenance root max dedup | neutral ordered graph builder 首个 persisted artifact fixture + 22-case original-builder fact/CSR/wormhole/provenance/config matrix 通过 | Authority unordered membership + directed evidence roots；source closure 与独立 root mass 属于 Nous ontology 适配 | nous-native |
| bounded Sense | memo_sensing sense_typed | energy、momentum、hop、previous-node state；normal/wormhole decay、立即回流抑制、FIR、邻居/state 上限 | Wave 的固定 outbound、步数预算与 state 上限；无 VCP wormhole/decay 合同 | neutral Sense 数值与状态 matrix 通过；单一 lineage 边界单独记录 | VcpQueryObservation 执行 neutral Sense，使用真实 graph/query policy 与 plan work bounds | nous-native |
| query river | memo_sensing SenseOutput | node potential 与实际注入 edge flow；source field 为最终 retained FIR 分布；edge 记录先于 momentum/state admission | QueryRiver 包含 source/potential/edge/provenance/mass；native source 为初始归一 seed | Sense node/edge/source field 与逐跳 transfer matrix 通过；VCP 单一 lineage 不作为稳定合同 | 同一 immutable observation；summary 保存 edge/node/hop/completeness，未测 discarded mass 为 null | nous-native |
| dual fields | memo_pipeline solve_dual_fields | 同一 source/transport 的两个 scaled resolvent，独立 L1 convergence、mass-ratio domains 与 vector projection | 尚无该 VCP profile 的双场 readout | neutral 双 resolvent 首个 native fixture、field vector projection 11 个 native case 通过 | 同一 source/transport 的 local/transfer readout，typed query policy 控制尺度 | vcp-derived |
| query gating / fusion | memo_pipeline gate_tags / fuse_observation | EPA/pyramid 激活、core/language/layer 门控；seed max、emergent cap、传播后 core/ghost、20% dedup 权重转移与向量融合 | native 使用 own sensing/activation | gating 16-case、fusion 17-case native 中间值对照通过；完整 query composition 待接入 | 同代 Tag/labels 与 explicit core cue 输入；typed query policy，ghost vector 不伪造 | vcp-derived |
| DTSC | memo_dtsc score_curve / run | ordered curve 的 exact/interpolated field contacts、coverage、continuity、action、closure、D/S/T、受限 reward floors；low trust 保留原输入顺序 | 当前 native 以 node potential 排序 | field 14-case、完整 curve/reward/batch fallback/order 22-case native 对照通过 | 同一 observation、真实 offered candidates、独立 DTSC readout、固定 Runtime fusion/Authority validation | vcp-derived |
| relative topology | rivermemo_topology_v3 evaluate_topology | exact/semantic node 对应，hop/position 相对距离、方向、独立 file 来源比例；node-only reliability cap；motif 复用 edge score | 当前 native node-potential 没有此 candidate readout | neutral 13-case native node/edge/component/reliability 对照通过 | V3 candidate root view 将 underlying occurrence/source roots 的 mass 分为 self/other | vcp-derived |
| morphology | rivermemo_topology_v3 compute_query_morphology | candidate/text 无关的 river 统计，三个 logits stable softmax，按样本/complete confidence 向 uniform prior 收缩 | 无三形态混合 | neutral 19-case native features/probabilities/discrete mode 对照通过 | 同一 VcpQueryObservation 的 candidate-independent query shape | vcp-derived |
| Ω | rivermemo_topology_v3 compute_omega | edge activation、emergence、正 raw flow entropy 的 epsilon-floored geometric mean，乘完整度因子 | native complete/discarded mass，尚无 Ω | neutral 19-case native Ω components/regime 对照通过 | 同一 numerical observation 的 component 计算；候选 readout 复用 | vcp-derived |
| Direct Anchor | rivermemo_topology_v3 compute_anchors | hop-0 seed/core exact 或高阈值 semantic contact，mass/specificity/closure/pool rarity noisy-OR；缺少 lineage 的 fallback 限 reliability | exact lane 与 native seeds，不等于 VCP 独立 anchor reward | neutral 11-case native contact/pool rarity/noisy-OR/reliability 对照通过 | V3 readout 使用 offered candidate/curve/pool 与同一 observation | vcp-derived |
| conditional innovation | rivermemo_topology_v3 assign_v3_scores | 条件 peer Gaussian 期望、variance/ESS uncertainty，正 innovation 经 role cap 与 Ω gate；独立 batch anchor activation | native topology rank 经 Runtime fixed fusion，无此读出 | neutral 17-case native peer statistics/role caps/anchor activation 对照通过；V3 candidate readout 已组合，12-case full-query numerical composition 已通过 | V3 candidate-specific root view、同条件 peer statistics 与同一 observation | vcp-derived |
| candidate observables / pure score | rivermemo_topology_v3 evaluate_observables / run_native | source contact、semantic boundary、双场覆盖/potential、closure；original/local/transfer cosine 与 bounded path reward | native 使用 own node-potential | observables 14-case native 对照、其中 5 个实际 selected curve 的 pure score 对照通过；另有 11-case 组合 readout | VcpReadoutCandidate + same observation 的 independent readout；BM25 为 reciprocal lexical rank proxy | vcp-derived |
| candidate pool / V3 readout composition | rivermemo_topology_v3 select_superset / run_native | 七路 channel quota/归一化/union score/source count；同一 observation 的 path/topology/anchor、形态混合、条件创新、最终排序 | native 使用 own TopologyWave lane | pool 12-case、cached observation 到完整 V3 readout 11-case native 对照通过；12-case full-query numerical composition 已通过；production adapter 已接线 | 真实 same-generation candidate index offers + lexical/dense/exact；Kernel current capability/freshness fences | vcp-derived |
| unified query preparation | memo_pipeline run_pipeline | 同一个 query 的 EPA/pyramid/gating/Sense/fusion/dual fields/projection，供 DTSC/V3 共享 | 已有 native shared signals 与 frozen native QueryObservation | 首个 full native observation + 12-case vector/config composition matrix 通过；ANN 返回次序作为条件输入 | 实际 Kernel VCP lane：cached one-query embedding、typed query policy、one immutable observation | vcp-derived |
| TimeDecay | RAGDiaryPlugin modifiers / _applyTimeDecay | 显式修饰符启用，V3 后、外部 rerank/截断前，以文本/路径日期作半衰期乘法；time lane 跳过乘法；native timeScore 仅进入 superset | Runtime own temporal hard constraints/age preference，不声明 VCP parity | owner 已确认；独立 temporal 实验待完成 | Runtime hard constraints 已实测；未复刻 VCP plugin 的文本日期乘法，独立 temporal 对照仍待完成 | nous-native |

## 当前数值 conformance

独立 checkout 的 frozen native 模块已编译。只读 research export 暴露同一 cached observation；条件创新 probe 另记录原函数局部统计值，原 score 表达式保持不变。原 blob、instrumentation 与 binary checksum 存在 ignored `data/research/vcp-reference/e03b891d42055cdf3cab5dc961cc71dd5facd65a/source-manifest.json`。

首个三维图 probe 实际执行 EPA publish、intrinsic residual、artifact build、统一 pipeline、DTSC 与 V3，导出 mean/basis/energy、residual/anchor、CSR、Sense 转移/node/edge、双场、morphology、Ω 与 candidate components/order。Tracked [数值 fixture](corpus/vcp/vcp-epa-dual-fields.json) 只包含生成的中间数值与来源身份，独立 [reference kernel](../../crates/retrieval/src/reference/fields.rs) 使用中性 DTO。

当前通过 query EPA 的 entropy/depth/resonance/axis energy，以及双场每个 node mass、支持域、迭代次数、convergence 与 L1 residual；absolute/relative tolerance 均为 `1e-10`。另已通过 20-case [EPA/双场边界矩阵](corpus/vcp/vcp-epa-field-boundaries.json)：orthogonal/duplicate-heavy/dominant/weak/single-axis/zero-centered/empty basis/translated query；empty/disconnected/immediate-return/fork-two-root/unknown-negative source；alpha clamp/zero/equal/near-unit、iteration minimum、support ratio 和 tolerance floor。Probe 直接调用 frozen `analyze_epa` 与 `solve_dual_fields`，显式构造 in-memory EPA cache 与 neutral CSR，没有替换原算式。比较 entropy/depth/resonance、cache availability、公开 dominant-axis energies、每个 field/domain、iterations/convergence/L1 residual；abs/rel `1e-10`。它不测 SVD training，也不宣称观察到 native 未公开的低能量 axis probability。另有 [Sense graph matrix](corpus/vcp/vcp-sense.json)，由 frozen `sense_typed` 实际执行生成 chain、fork、merge、立即回流、hub、两独立 roots、wormhole bridge、disconnected distractor、weak/strong edges、state truncation、empty seeds 和 transition-record truncation 共 12 个 case。

独立 [Sense kernel](../../crates/retrieval/src/reference/sense.rs) 逐值比较 source field、node energy/normalization/hop、edge flow/max/normalization/conductance、逐跳 transfer、抑制质量与状态截断数量，absolute/relative tolerance 为 `1e-12`，离散 membership/flag exact match。数组以 node/edge key canonicalize，保留全部中间数值。Frozen 合流 state 的单一 seedId/originType 由首次 HashMap encounter 保留，不能宣称稳定唯一 lineage；reference 数值 DTO 不输出这份任意 lineage，candidate adapter 需要区分实际来源。

独立 [ordered graph builder](../../crates/retrieval/src/reference/graph.rs) 以 file/tag/position、pair similarity、anchor gain 和 typed config 为中性输入。[graph fixture](corpus/vcp/vcp-graph.json) 对照 frozen native 的 30 条归一化前 fact entries、CSR 节点/行/目标/conductance、wormhole 集合、inbound mass 与逐文件 provenance，容差 `1e-12`。Fact export 只调用原 private builder，算法函数没有修改。

图构建的 reverse anchor/semantic/distance 影响 fact mass；log evidence、hub correction 与 wormhole reserve 影响预算内 transport。Provenance 使用单独的 file/direction/distance mass 合同，独立计算，避免把 transport 权重当来源质量。另已通过 [graph builder matrix](corpus/vcp/vcp-graph-boundaries.json) 的 22 个原函数调用 case：chain/fork/merge、立即回流、generic hub、双独立 roots、wormhole reserve、disconnected distractor、weak/strong semantics、unordered/equal/sparse positions、duplicate Tag、forward/reverse/semantic/transport 参数边界，以及 empty/single/oversized-file filtering。直接比较 fact mass、CSR、wormhole 分类与 file provenance；abs/rel `1e-10`，离散结构 exact。该 call adapter 不导出派生 inbound view，inbound 的 persisted artifact 对照仍由首个 fixture 覆盖。

[Residual Pyramid fixture](corpus/vcp/vcp-pyramid.json) 固定 actual pipeline 的每层 ANN 返回候选，覆盖 duplicate directions、weak residual、dominant direction 与 empty query。独立 [Pyramid kernel](../../crates/retrieval/src/reference/pyramid.rs) 按该顺序正交化，检查每个 Tag contribution/handshake magnitude、projection/residual magnitude、energy ratio/explained、direction coherence/pattern/noise，以及最终 depth/coverage/novelty/activation。弱残差 case 实际经过三层，避免单层满秩例子掩盖层间合同。

[Query shape fixture](corpus/vcp/vcp-query-shape.json) 使用 frozen `compute_query_morphology` 与 `compute_omega` 的只读 export，包含 Sense matrix 的河网以及独立深链、同层关系、空/不完整观测、单边、零 raw flow/正 normalized flow 和 Ω scale override，共 19 个 case。独立 [query shape kernel](../../crates/retrieval/src/reference/query_shape.rs) 对照所有形态 features、confidence、三个概率、dominant mode，以及 Ω edge/emerge/flow/最终值与 regime；数值容差 `1e-12`，离散结果 exact match。

Morphology 的正 normalized flow 可以形成方向统计；Ω entropy 使用正 raw flow。这个边界已有单独 fixture。输入不包含 query text 或 candidate，保证这两个 kernel 的测量对象是 query observation。

[Direct Anchor fixture](corpus/vcp/vcp-anchors.json) 从 frozen `compute_anchors`/`anchor_contacts` 导出 11 个中性曲线 case，包括 exact/semantic、fallback、无 inbound、零 mass、缺 seed vector、semantic discount、全池共同 anchor、稀有 exact anchor、负正文 closure、空 seeds/pool。独立 [anchor kernel](../../crates/retrieval/src/reference/anchors.rs) 对照 score/reliability/strength、exact/semantic/contact 数与 mean closure，容差 `1e-12`。Rarity 先读取完整 selected pool，之后各候选独立读出；semantic cosine 用于选择 contact，奖励使用固定 discount。Hop-0 seed/core 的来源筛选仍由统一 pipeline/adapter 负责，不能将此 kernel 接受的任意 seed DTO 当作真实直接来源证明。

这些 fixture 证明给定候选序列时的数值 kernel；ANN 搜索排序和 ontology mapping 仍属于 adapter utility。[Path geometry fixture](corpus/vcp/vcp-path.json) 从 frozen `evaluate_path` 生成 10 个 case：正反有序曲线、transfer-only bridge、支持域外尾部、缺失 conductance、单 Tag、空曲线、负正文 closure、权重/closure override 和空 fields。独立 [path kernel](../../crates/retrieval/src/reference/geometry.rs) 复用中性 Curve DTO，按 max-normalized field 读取相邻段，逐值比较 local/transfer potential、direction、continuity、support/transfer count、正文 closure、path core/quality，容差 `1e-12`。

支持标记由有效域和边存在性计算，质量还取决于实际 field mass；因此支持 count 非零不等于 path quality 非零。单 Tag 使用受限节点读出，仍保留段 count 为零。此处验证数值 kernel，未将正反 case 的差值外推为实际 retrieval utility。

[Relative topology fixture](corpus/vcp/vcp-relative-topology.json) 通过 frozen `evaluate_topology` 导出 13 个中性 case，涵盖正反/semantic 曲线、多个 query nodes 对应同一 candidate tag、node-only/无对应、self/independent source、weak edge/cap、distance/direction override、负正文 closure 和空 river。独立 [topology kernel](../../crates/retrieval/src/reference/topology.rs) 对照 node/edge coverage、alignment、distance/direction/edge/motif 分量、两个 graph heads、最终 score/reliability/mode，容差 `1e-12`。来源贡献按 file ID 读取独立比例，保留 frozen `0.15` self-evidence floor。

当前 motif 分量与 edge topology 分量严格相同，没有额外图同构判定。缺少完整 edge 对应时采用 node-only readout，并限制 reliability；分数与 reliability 分开记录。

graph builder 的上述边界矩阵已完成；full query composition 的上述矩阵已完成；实际 artifact lifecycle 配置范围及 TimeDecay 对照仍待执行；EPA basis builder、intrinsic residual/gating/fusion、field projection 和 candidate readout 的已有矩阵见上表。Adapter utility 以实际 benchmark 为准，不能以数值 parity 代替。

独立 [V3 scoring head](../../crates/retrieval/src/reference/scoring.rs) 与 [compact fixture](corpus/vcp/vcp-scoring.json) 比较实际函数记录的 peer expectation、variance、ESS、uncertainty、positive innovation、candidate/statistical confidence、requested bonus 和 peer count。Atomic/propositional/narrative、单候选/两候选/空池、Gaussian fallback、direct frontier、thematic cap、低/零 Ω、batch anchor promotion/平滑激活/饱和以及参数覆盖均通过 `1e-12` absolute/relative 对照，角色与数量 exact match。Anchor promotion 与 anchor reward 是两个独立决策：默认 z=2 的五候选案例中，最强 anchor 可提升角色而未超过激活阈值，加分仍为零。此组以 precomputed pure/graph/closure 等中性标量为输入；完整 candidate observables、pure score、候选池构建及 Nous adapter 尚未由此组证明。

独立 [observables / pure score](../../crates/retrieval/src/reference/observables.rs) 另使用 [compact fixture](corpus/vcp/vcp-observables-pure.json)。五个同一次 native run selected curve 使用数据库实际 f32 Tag/chunk 向量和 cached original/local/transfer vector，逐项比较 observables 与 pure score；九个附加 observables case 覆盖 hidden direct、零/反向 query、空 curve、无 source、无场、tail-only、local-only 和重复 source ID，使用原 `evaluate_observables` 直接生成期望，均以 `1e-12` 对照通过。此组的 geometry/topology/morphology 为条件输入，尚未证明候选池选择、完整 graph mixture 或统一 candidate 流水线。

独立 [candidate pool](../../crates/retrieval/src/reference/pool.rs) 的 [compact fixture](corpus/vcp/vcp-candidate-pool.json) 比较 native `select_superset` 的来源、union score、quota、cap 和次序。独立 [V3 readout](../../crates/retrieval/src/reference/readout.rs) 再从同一 cached observation、完整 offered curves、持久 f32 向量和冻结 CSR/provenance 计算全部 candidate 读出；[compact fixture](corpus/vcp/vcp-v3-readout.json) 使用实际原生 `run_native` 调用，覆盖默认、source quota、union cap、最终截断、BM25/time 补充、无 seed lineage、node-only river、不完整观测、空双场、visible direct 范围与联合参数覆盖。Morphology/Ω、selected pool、最终 membership/顺序、geometry/topology/observables/anchor、pure/final score 和三种 bonus 均通过 `1e-12` 对照；baseline 为六个 offered、五个 selected，最终次序 `4,1,6,2,3`。

这个组合组的 query observation 与图资产是冻结条件输入，没有重复执行 Sense 或求场；EPA training、intrinsic residual、query gating/fusion/projection、DTSC 和 Nous production adapter 仍未由这组完成。Native `allowedFileIds` 在此 readout 中仅控制 observable direct visibility，不能作为 Nous Authority 的权限证据；adapter 必须消费经过 Authority 限定的候选与图视图。

独立 [DTSC field preparation / sampling](../../crates/retrieval/src/reference/dtsc/field.rs) 使用 [compact fixture](corpus/vcp/vcp-dtsc-field.json)。期望值由原生 `run` 的实际局部场数据及原 `sample_field` 生成，比较完整 exact field、保留 interpolation nodes/向量、总能量/最大值/熵、trust flag 和采样势能/exact/source type。案例包含质量与数量截断、插值子集外 exact contact、sourceField-only、空场、低熵及关闭熵守卫、缺向量、维度不符、energy fallback、重复 node、配置夹逼与负语义阈值，均通过 `1e-12`。

独立 [DTSC curve kernel](../../crates/retrieval/src/reference/dtsc/curve.rs)、[typed config](../../crates/retrieval/src/reference/dtsc/config.rs) 与 [batch readout](../../crates/retrieval/src/reference/dtsc/mod.rs) 已接入完整场准备。[compact fixture](corpus/vcp/vcp-dtsc.json) 由实际 `rerankMemoDtsc` 生成，核对全部数值 curve/reward/diagnostic 字段与完整 membership/order；Tag 文本标签由 adapter 映射，不作为中性 numerical kernel 输出。案例覆盖 native defaults、实际 rag 配置、低支持/低熵/缺向量/无接触回退、sourceField-only、emergent structural、两个 semantic direct contacts、thematic、非零 sparse pair relief、非零 geometry/identity floor、联合低覆盖/低区分度、缺失曲线、Top-K、空候选、维度错配、零维度拒绝和冲突参数夹逼。数值 tolerance 为 `1e-12`，membership/enum/count exact；零维度按输入错误拒绝。

DTSC 的正常最终分为原 KNN 分加 bounded reward，不沿用 V3 的 `[0,1]` final clamp。批级回退清零 base/aux/total bonus 并保留输入次序；`fallbackToKnnOnLowTrust=false` 关闭的是联合低信任/熵守卫，最小支持、向量可用性和无贡献回退仍存在。此组资产与查询观察为冻结输入，query preparation、完整矩阵要求与 production adapter/benchmark 仍需完成。

独立 [Intrinsic Residual builder](../../crates/retrieval/src/reference/intrinsic.rs) 使用 [compact fixture](corpus/vcp/vcp-intrinsic.json)，由实际 native task 在独立 SQLite 文件计算并读取持久 value/status。包括 anchored GS/centroid/SVD、basis 限制、未归一化输入、缺少 pairwise、关闭语义门控、hard floor、min gain、抵消 centroid、零向量、空/超大 file、legacy position、固定 anchor mapping 与配置夹逼。ID、status、neighbor count exact；raw ratio/anchor gain 的 absolute/relative tolerance 为 `1e-10`，SVD 使用 f32、不同 nalgebra 版本的 `1e-6` tolerance。没有把全库 median 引入 anchor mapping。

Frozen task 的等权邻居顺序来自 HashMap；独立 builder 用稳定 ID 起序，不声称任意 tie 的唯一 basis/lineage parity。资产签名、数据库 cache/persistence 属于 adapter/lifecycle，数值 fixture 没有替代这些生产要求。

独立 [field vector projection](../../crates/retrieval/src/reference/fields.rs) 的 [compact fixture](corpus/vcp/vcp-field-projection.json) 实际调用原 `project_field`，覆盖 local/transfer 场、单节点、缺失 index node、空/非正场、抵消、微小向量、未归一化向量和质量缩放。按 artifact node order 累加可用 f32 index vectors，只用可用节点质量取均值，再作 f64 单位化和 f32 输出；absolute/relative tolerance 为 `1e-7`。

独立 [Tag gating](../../crates/retrieval/src/reference/gating.rs) 的 [compact fixture](corpus/vcp/vcp-gating.json) 直接调用原 `gate_tags`，核对输出 seed ID/name/weight/core、effective boost 与 dynamic core boost。覆盖技术/社会/Unknown world、语言开关、core casefold、零相似度回退、非正节点、首次重复占位、空 pyramid、range fallback 与参数夹逼，数值 tolerance 为 `1e-12`。

独立 [fusion](../../crates/retrieval/src/reference/fusion.rs) 的 [compact fixture](corpus/vcp/vcp-fusion.json) 使用原 `fuse_observation`、实际 index vectors 与 SQLite core-name lookup。核对 seed max/emergent 排序与 cap、core 补全、hard/soft ghost 和无效 ghost、缺/零向量、20% dedup 转移与 core 属性、全部诊断计数/selected IDs/weights，以及最终向量，数值 tolerance 为 `1e-12`。空 selected 保持原 query 而非强制单位化；已在 emergent 中的 core ID 不被补全重复升级。以上仍为 component fixture，完整 query pipeline 与 Nous adapter 尚待接入。

独立 [EPA training](../../crates/retrieval/src/reference/epa_training.rs) 的 [compact fixture](corpus/vcp/vcp-epa-training.json) 核对原 `select_epa_density_residual_samples` 和实际 native compute/publish cache。案例包含冻结十 Tag、六维密集分布、anchor 数量上下界、单 basis、未归一化输入、七 Tag 不足和十二维能量截断。密度 mean/key、centroid、weights、labels、representative count/bucket count 与 weighted mean 对照通过；sampling f32 tolerance 为 `1e-7`，SVD basis 以符号等价、energy 以 absolute/relative `1e-5` 比较不同 nalgebra f32 实现。十二维案例实际保留 11 axes。

Sampler 的 xorshift 12-bit density key、残差/密度评分、多样性衰减、candidate `swap_remove` 次序、f32 加权 SVD 和 95% 能量/minimum-eight-axis 规则均由独立实现承担。Representative samples 仅影响诊断数量，不被追加为 SVD 行。参数化 `samples_per_anchor` / `candidate_limit` 对应 native 环境参数，本组验证其默认 32/512；同分 bucket 的 native HashMap 起序未声明稳定 parity。Basis 资产发布与 generation 元数据仍属于 production adapter/lifecycle 的后续工作。

独立 [query pipeline](../../crates/retrieval/src/reference/pipeline.rs) 的 [首个组合 fixture](corpus/vcp/vcp-query-pipeline.json) 使用同一次实际 native `run_pipeline` 输出，从 query/cache basis/Tag vectors/图资产进入 EPA、Pyramid、门控、Sense、融合、双场及投影。EPA stats、Pyramid/gating、完整 source/node/edge 数值、Sense 诊断、fusion 计数/selected IDs、双场每节点质量/domain/convergence/residual 对照通过；f32 enhanced/local/transfer vector 使用 `1e-7` tolerance。Native 任意合流 lineage 不作为唯一稳定来源输出；ANN 实际返回的候选次序与相似度仍是冻结输入。此组只覆盖首个组合案例，剩余查询矩阵、生产索引 adapter/资产生命周期、Authority 映射和 benchmark 待完成。

## Nous adapter 映射进度

[VCP adapter identity/evidence boundary](../../crates/retrieval/src/vcp/adapter.rs) 已实现 generation-local `CognitiveRef` 双向映射、中性 candidate curve 与 reference ranking 到 LaneCandidate 的转换。输入引用稳定排序并去重，ID 从 1 起；输出引用必须属于同一映射，未知/非正 ID 拒绝。Curve member 使用同维、有限 embedding，保留真实 source sequence；无 Authority 序位时明确使用 `stable_identity` 控制顺序，不能据此宣称 narrative/causal order。

Positive AssociationEvidence projection 保留 support class、association kind 与独立 provenance root，按 directed edge/root/class/kind 的重复输入取最大 support mass，不把重复记录当作独立支持；negative evidence 不产生 reference 正向 flow。此变换不写回 Authority，不改变 acceptance/evidence 语义。最终 ranking 去重并映射原 CognitiveRef，保留 kernel metadata；这些 ID 映射检查不能代替 Runtime Authority revalidation。

映射聚焦检查覆盖输入次序不变的 identity、source/stable curve 顺序、重复 member、维度错误、独立/重复/negative evidence 和未知输出 ID。Production graph/embedding 资产组装、Typed config、atomic generation publication、immutable VCP observation、引擎与 public query 接入仍待完成；当前 VCP profile 尚未启用。

`AuthorityStore.cognitive_projection_input()` 在同一个 repeatable-read read-only transaction 中读取 subject Authority sequence、拓扑证据和符合 capability 的 material/memory/longitudinal 文本源。原 `topology_projection_input()` 复用同一个 topology snapshot owner；`CognitiveProjectionInput.authority_watermark` 表示全 Subject sequence，另有原 topology watermark，二者不能互换。

[Serving VCP material preparation](../../crates/retrieval/src/vcp/material.rs) 复用既有文本拼装与 host-supplied embedding provider，冻结 space/producer 并检查每个输出身份/维度。它返回 generation identity map、独立证据、candidate vectors 与 Tag membership，Authority 无序 membership 明确标记 stable-identity 曲线顺序。缺已存储 embedding 时返回 unavailable；本机配置使用 StoredEmbeddingProvider，该步骤没有独立远程调用路径。

真实 PostgreSQL 的 association integration 回归同时验证 memory 开关对文本与拓扑的同步排除、独立 association provenance、缺 material、提交存储 embedding 后的候选 vectors/space/producer 和全 Authority 水位。此准备入口现在由 VCP profile generation build 消费；查询 lane/readout 仍待接线。

[Nous graph assets](../../crates/retrieval/src/vcp/graph.rs) 将同一 projection 的 Tag membership 和正向 Authority evidence 汇入独立 reference transport。无序 membership 使用 position=0 的对称 cooccurrence 分支；有实际 source sequence 时保留位置。Evidence 先按端点/独立根/support class/association kind 取最大值，再累加独立贡献；保留原 semantic identity 与质量。这里未添加新的 support-class 权重。

Reference 图 owner 现在分开构建按文档的 facts 与全图 transport，批量共享 pairwise/anchor lookup。Adapter 的 provenance root ID 使用独立命名空间，每条边保留各根贡献，根表可回译到原始 identity。它还不是 V3 candidate visibility 的 file ID；查询接线时必须由 Authority scoped candidate view 映射可见贡献。Frozen ordered graph parity 和 adapter 的无序/去重/重排回归通过，真实 PostgreSQL material 也已构建 transport。该资产构建入口现在由同代 VCP generation 消费；VCP 查询 readout 仍不可用。

[VCP generation owner](../../crates/retrieval/src/vcp/generation.rs) 持有 identity map、embedding space/producer、candidate/tag vectors 与 labels、曲线及顺序来源、cooccurrence pairwise、intrinsic residual/anchor、EPA basis 和 graph assets。缺少 concept vector 时返回 unavailable；图中缺向量的节点显式保留在 diagnostics 资产中。EPA 标签不足时保留 reference 的 cache unavailable 状态。

`retrieval.vcp.assets` 是 Developer/SystemOnly/ServingRebuild 的 typed policy，默认 EPA anchors/max basis 为 64/64，samples/candidates 为 32/512；图与 intrinsic 参数来自明确的独立数值合同。VCP `vcp.json` 使用既有 staging readback/checksum/rename/publication 流程，回读检查 generation/identity map、vector/curve identity、EPA shape 与 provenance root。ServingSnapshot 的 native/VCP 视图在同一次 publication 中互斥切换；topology implementation revision=6。VCP 配置 digest 包含 profile、asset policy、synopsis budget、space/producer 和 capability，刷新水位使用完整 Subject authority_seq。

真实 PostgreSQL 回归验证 VCP publication/reopen、native/VCP 切换、回读拒绝乱序 identity map 和曲线向量不一致。Asset policy 的 outbound mass 从 0.95 改为 0.7 时 generation 重建，每个非空 transport 行总质量变为 0.7。此次验证覆盖资产生命周期；immutable VCP query observation、候选 readout、Authority scoped provenance visibility 和公共查询仍待完成。

[VCP indexed generation](../../crates/retrieval/src/vcp/index.rs) 将数值资产与 candidate/Tag 两个 USearch index 放在同一个不可变 owner 中。三个文件使用同一次 staging/checksum/publication；回读逐 ID 检查 index vector 与原资产一致。VCP 索引显式使用 cosine metric，Dense 原有 metric 选择保持原合同。Tag residual 搜索使用 frozen pipeline 的 f64 `1/(1+f32 distance)` similarity，并从同代 label/vector 表返回输入。实际 USearch 保存/回读后，两个正交 Tag 的 residual 搜索结果一致；真实 PostgreSQL generation 的 candidate 搜索包含 MemoryRevision，空 Tag index 返回空输入。这里只证明索引接线，未声明 ANN 排序与 frozen VCP index 完全一致。

[VCP request observation](../../crates/retrieval/src/vcp/observation.rs) 持有私有 query/profile/generation/config identity、bound time、五轴时间上下文、原 embedding 和一个完整 numerical pipeline output。它借用同代 Tag index，仅运行一次 EPA→Pyramid→gating→Sense→fusion→双场与投影；无候选排序、provider 调用或 mutation API。Tag cues/exact Tag bindings 映射同代 core labels，未生成未绑定的 ghost vectors。Profile、asset policy、embedding space/producer 不匹配以及 Forbidden embedding 会拒绝构建。Frozen Sense 最低 admission cap 为 100；更小的 bound budget 返回 unavailable，其他预算按 QueryPlan 限制。

`retrieval.vcp.query` 是 Developer/SystemOnly/Live/QueryPolicy 的 typed policy，默认值来自独立 numerical contracts，包含 Pyramid/gating/Sense/fusion/fields。真实 PostgreSQL 回归验证缺 EPA/空河网路径、producer/policy fences 与 observation 的 generation 不变；独立 lab material 经真实 index 构造非空 Pyramid/Sense/双场和 core Tag 映射。FIR paired probe 在同一 lab generation/中文 Tag 输入下仅将 gamma 从 0.6 改为 0.9，source field 改变而 Serving generation 不重建。最初英文 Tag fixture 在 Unknown world 下 language confidence=0.05，未达到 firing threshold，单节点归一化源场相同；该输入对 FIR 不敏感。

修正 shared query signals 仍依赖 Dense generation 的接线缺陷：存在匹配 VCP generation 且允许 embedding 时，可以在 Dense 未启用/没有 Dense generation 的情况下准备一次共享 embedding；Forbidden 仍不调用 provider。实际 VCP-only index 回归覆盖此路径。当前 observation 入口可执行，DTSC/V3 数值 readout 已可执行；公共 topology lane 与 Authority candidate/self-root view 仍未接通。

[VCP readout adapter](../../crates/retrieval/src/vcp/readout.rs) 的 DTSC/V3 函数借用同一个 immutable observation，接受调用者提供的 offered candidate、base/BM25/time/anchor signals 与 self-evidence roots。它检查 generation/profile、ID/curve、重复候选、有限数值和已知 root，不查询模型、不扩展候选或赋予权限。DTSC 输入直接使用 EPA/Pyramid metrics，depth 交给 frozen kernel 的 clamp；hop-0 来源由实际 gating core/seed 标注，后续节点为 emergent，未构造不稳定的 seed lineage。

V3 支持可选 per-candidate provenance view，旧 reference file-owned view 保持默认合同。Nous adapter 将每条边的实际 root mass 按该候选的 canonical document root 和声明 self roots 分为 self/other 两类，只传质量比例给原 independence 公式；两个质量 token 不代表新证据根。这样同一真实根可以属于多个候选，且不重复增加总质量。View 中 offered candidates 已由调用者决定，reference visible 标记仅控制 observable direct，不能代替 Authority 验证。

`retrieval.vcp.readout` 是 Developer/SystemOnly/Live/QueryPolicy typed policy，包含独立 DTSC/V3 numerical config。实际 index→observation→两种 readout 的 lab case 返回原 CognitiveRef，并确认 observation 不变；将已知独立根加入候选 self roots 后，V3 edge topology score 降低，未知 root/重复候选被拒绝。20 项 frozen reference 检查继续通过。完整公共查询仍待当前 Authority 候选/self-root 图视图和 topology lane 接线，此处没有检索质量或权限完成声明。

Coherent projection 现在在同一个 repeatable-read transaction 中解析每个 source 的 evidence root closure，并随 candidate material/同代 generation 保存。Memory/schema 复用 association 的来源解析；Episode supports、Journal sources、Occurrence、SourceRegion 与派生 representation 的 source ancestry 进入闭包。重复访问同一依赖不生成额外 unknown root；没有可解析来源时保留明确 unknown-dependency。SourceRegion 通过同 Subject 的 artifact occurrence 对齐 external/artifact/occurrence 根。

V3 self-mass 计算自动消费 generation 的 candidate evidence roots，并保留调用者提供的补充 self roots。真实 PostgreSQL 回归验证 Memory 有实际来源根、memory capability 禁用时对应 root entry 被移除，以及 embedding material 保留来源集合。Generation readback 校验 root entry 的 candidate IDs。此更改完成快照内来源 ownership 接线；当前 Authority 查询候选过滤、过期图处理和公共 lane 仍待完成。Episode/Journal/derived 闭包分支尚未在本轮增加专用 fixture，不据此宣称全部矩阵完成。

V3 candidate provenance 现在将 document contribution 回译到 generation 保存的来源闭包，避免同一 underlying source 的多个文档贡献全被当作独立来源。一个 document mass 在其来源根间等分；与候选自身来源重叠的份额进入 self mass。Unknown/空文档来源不提供 independent credit，但仍保留原 transport 贡献质量。明确的独立 Authority evidence root 保持原 root mass；每条贡献的 self+other 总量不变。该分摊是可辨认的 Nous ontology adaptation，不声明 frozen VCP file ontology parity，也未改变 reference independence 的 0.15 floor。专用回归覆盖完全重叠、部分重叠、unknown/空来源和显式根；现有实际 index→两个 readout PostgreSQL 回归继续通过。

[VCP public lane](../../crates/retrieval/src/vcp/lane.rs) 已由 SharedLaneProvider 根据 bound profile 分派。DTSC 从原向量 index 提供候选；V3 从原/增强/local/transfer 向量 index 合并 offered candidates，并加入现有 lexical/dense signals 和 exact bindings。Index 搜索前按 query domain 过滤；base candidate 质量/可见性最终仍由 Runtime 的固定融合及 Authority revalidation 控制。Lexical source 目前使用现有 lane rank 的 reciprocal 作为 pool signal，time source 为零；这些是明确的 adapter 映射，未伪造 BM25 或日期分数。

数值 observation/readout 前后检查完整 Subject authority_seq，并检查实际 memory capability；过期 fallback artifact 不生成候选。Profile/asset/embedding identity fence 仍由 observation owner 执行。Source-field 非空、state/transition truncation 与双场收敛分别可观察；discarded state mass 当前没有数值测量，truncation diagnostics 明示这一点。

真实 PostgreSQL Kernel query integration 已验证两个 VCP profile 返回 MemoryRevision 并记录实际 profile/ready lane；通过普通 suppress lifecycle 改变 Authority 后，直接复用旧 generation 得到 unavailable/空候选，restore 后继续；Forbidden embedding 不生成 VCP observation。Native golden 和 20 项 frozen reference 检查通过。此结果验证 Kernel Runtime 查询接线；随后真实 provider HTTP probe 见下文，CC0 category benchmark 的 no-rerank 首轮及真实 context 第二轮已完成，paired model rerank 与外部 suites 仍在运行，见 [corpus](corpus/README.md)。

在 `a2303cc` Core/Kernel 运行时已完成真实 New API HTTP probe；该旧实例目前已停止。已有 ASR Subject 的同一 text query 在 native/DTSC/V3 下均返回 2 hits，无 degradation；该 Subject 无 Tag 源场，只证明空场路径。新增原创 CC0 观测站维修场景（3 Memories、3 Tags）通过普通 observe/form API 提交，并由真实 provider 生成 embeddings；同一 text+Tag query 下 DTSC/V3 各激活 8 edges、访问 6 nodes、最大 hop=1，返回 3 Memories，无 degradation。Native 激活 6 edges、最大 hop=2，同样返回 3 Memories，但报告配置预算截断。

六组最终输出均经过既有 model rerank；正场三组顺序一致，未形成算法质量结论。新 gateway observation 中 10 个实际 /embeddings 或 /rerank 调用均为 HTTP 200；30 个 trace JSON 文件的 prohibited header/实际 credential 字节扫描为零命中。原 cognitive profile 已恢复。完整 category benchmarks、消融、无 rerank 对照和更深传播场景仍待执行；3 条原创 Memory 只是后续 CC0 benchmark 的初始场景，未达到 Spec 规模。

前缀 benchmark 发现并修复 topology serving 的跨 profile publication 缺陷：VCP 资产使用完整 Subject authority_seq，native 资产使用 topology 子水位，切回 native 时 persistence 的单调 publication fence 返回了水位较高的旧 VCP 资产。Topology snapshot 和 freshness 检查现在统一使用完整 Subject 水位，implementation revision=6；数值算法未变化。实际 PostgreSQL 回归明确制造 full seq > topology desired 的情况，验证 VCP→native 发布成功，native 视图和记录水位正确。Native golden 保持通过；缺陷前 benchmark 输出不参与正常算法评分。

## 需要保留的实现边界

### 观测与完整度

VCP 同一 request 的 Sense、增强向量和双场保存在同代 runtime observation 中，DTSC/V3 复用它。不同 artifact signature 清除 cache；同 signature publication 幂等，handle 有 generation fence、容量 256 与五分钟 TTL。Nous 使用 request-scoped immutable observation，无需复刻 N-API handle。

VCP Sense 的 source field 是最终节点 FIR 能量的总量归一值，并非 hop-0 seed。边流在 next momentum 检查之前记录；进入河网的注入不必进入下一 hop state。Trace transition 只保留被接受的目的状态，有独立上限。Native V3 从 cached observation 设置的 `complete_observation` 只检查 source field 非空，未结合 state truncation、transition truncation 或双场 convergence。Reference 应重现此语义，Nous diagnostics 应分别保存可观测事实。

### DTSC

Exact contact 读取完整正能量场；vector interpolation 另受 mass-ratio/top-node 支持集合限制。几何 auxiliary 与 identity anchor 是有界 reward floor，不是无条件相加奖励。Low trust fallback 清空 bonus 并保留输入顺序；只恢复原 KNN 分数再排序不符合该合同。

### RiverMemo V3

JS 控制面只转交部分配置。`TagMemoV10Engine.getEffectiveConfig()` 合并 laboratory/riverMemo 并冻结 nested defaults；原生资产构建保留同代 `orderedCooccurrence`、`v9`、`spikeRouting`。`RiverMemoEngine._nativeConfig()` 没有转交 pure original/local/transfer weights、role cap/multiplier maps 和 frontier 常量，对应 production Rust 固定值须与实验覆盖项区分。

完整源码表明 `TagMemoEngine.applyTagBoost()` / `geodesicRerank()` 在 Rust-owned 资产下被显式停用。其旧 `observeQueryForV10()` 使用 enhanced vector delta 判断完整度；当前 native cached observation 使用 source field 非空，不能混合这两种合同。Deep Dive 中关于私有概念和普遍收益的描述属于上游观测，本轮检索质量仍由独立 benchmark 判定。

Unified geometry probe 是离线研究程序：采样/裁剪图、忽略 association reserve 的简化 kernel、叠加 decay 的 transport、无生产 momentum/state cap 的单种子有限场，以及简化 curve 泛函。它的同泛函两乘两设计、去源归一化和随机/拓扑消融可用于实验设计，不能用其输出代替 production DTSC/V3 source conformance。

候选使用文件 Tag 的稳定序位；relative topology 分别比较节点、边、方向、距离与独立来源比例。当前 motif score 复用 edge topology score，没有单独候选 fork/merge 模体匹配。

Morphology 与 Ω 只读取 query river。Morphology 的 active degrees 来自正 flow，sample confidence 使用数组 node/edge count。Ω 的 edge component 使用全部 edge count，flow entropy 只使用正 raw flow；单条正 flow 为 0.5，无正 flow 为 0，各 component 在 geometric mean 前取 epsilon 下限。

Direct Anchor 的 semantic similarity 用于选择 contact；contact contribution 使用固定 semantic discount。候选池 rarity 依赖完整 selected pool。Conditional innovation 同时需要高于条件期望和 uncertainty，不能以裸 graph score 替代。Role caps/部分 frontier 常量在当前 Rust 中固定，不全部从 rag_params maps 读取。

V3 pure score 直接混合 original/local/transfer cosine，加有界 path reward；enhanced vector 用于 superset，不是第四个 pure semantic 分量。BM25/time 参与候选来源，不直接加入 pure final 分。Nous adapter 只能生成 TopologyWave lane 内的排序和可解释 metadata，最终仍由 Runtime fixed fusion 与 Authority revalidation 决定。

`::TimeDecay` 是 RAGDiaryPlugin 的后处理，位于 RiverMemo 返回后、外部 rerank/最终截断前。半衰期、最低分优先取修饰符，其次全局配置，fallback 分别为 30 天和 0.5；日期依次取 Tag 行、括号、首行、路径。对有效日期使用非负整数日龄，原分优先 `rerank_score`，乘 `0.5^(days/halfLife)` 后排序并按最低分过滤。`source=time` 跳过衰减乘法，仍参加全批过滤；未匹配 target Tag、无日期或日期无效的候选保留原分。此合同不等于 Nous 的多轴时间约束，后续比较须分别标识。

数值 goldens 保留独立的 frozen inputs/expected values；这些常量的重复有意保持，避免共享输入或生成 expected 使 parity oracle 与实现一起漂移。冻结数值已集中为 Research 数据，生产 reference/adapters 和测试逻辑继续审计。Reference kernels 的有限值 `[0,1]` 夹逼和非负标量规则已共用同一实现，全部数值 contracts 保持不变。


VCP Sense 当前测量 activated edges、visited nodes、seed count、max hop 和 truncation；没有测量被预算丢弃的 probability mass。`topology_discarded_mass` 因此为 null，native 则保留其实际 `discarded_state_mass`。此前 raw benchmark 的 VCP 值 0 是未测量占位，不能解释为零丢弃；当前进程仍运行旧 binary 的 raw 输出需按此说明读取。

## Unified pipeline 向量与配置矩阵

[compact pipeline matrix](corpus/vcp/vcp-pipeline-state-budget.json) 使用独立 checkout 的真实 `runMemoPipeline`、同一个 persisted artifact、实际 USearch index 和 cached observation。包含原 query、三个正交方向、弱残差、反向向量、单层 Pyramid、关闭语言 gate、较低 fusion boost、替代双场/迭代预算、FIR gamma=0.9，以及状态/transition 截断。每条同时对照 EPA scalars、Pyramid 中间值、gating、Sense source/node/edge/diagnostics、fusion、双场与投影向量。

Native ANN 返回序固定为 conditional input；这些 case 不证明 ANN recall parity。Sense 的单一 lineage 属性不进入 neutral output，原 source 上已确认的非稳定 lineage 边界仍保留。Scalar abs/rel `1e-10`、其他公开 component `1e-12`、f32 vector abs `1e-7`，并检查向量长度；没有为了排序一致而放宽中间值。它也不能代替真实 embedding space 下的 category utility 或所有 Configuration 的 causal probe。
