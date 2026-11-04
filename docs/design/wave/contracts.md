# Wave 公共值与处理结果

## 引用

WaveSnapshotRef 保存 instance_id、snapshot_id 和 wav 短地址。WaveCandidateRef 保存 snapshot_ref、candidate_key。解析引用由 Wave owner 完成；内容仍由 ArtifactRef 与 selector 读取。row_id 仅在该 NativeSnapshot 内有效，不成为长期用户地址。

## 构建请求

CollectionManifest 的 documents/concepts/source_sequences/candidates/support_groups/associations 见 assets。input 对象必须且只包含 file、text 或 artifact 一个来源；已有 artifact 可带 selector。DocumentInput.selector、CandidateContent.selector 与 ContextContent.selector 均为共同 SourceSelection，省略或 `{}` 表示全文；各字段沿用 content 的 start_line/end_line、start_byte/end_byte、pages、start_ms/end_ms、frame/crop。candidate.content 的 document 必须等于实际正文来源，candidate.document 用于曲线文稿分组；两者不同必须在 manifest 明确说明，不能自动用 evidence root 代替。

derived_from 只引用 documents key，构建读取 DAG 的终端来源。evidence_group 是集合内独立支持分组标识，不作为原件 ID。association.support_group 指向具有完整共同前提的支持组；负 polarity 保留，不送进正传输矩阵。

Concept.key 与 candidate.key 分别唯一，重复 label 允许；core 必须按 key 精确指定，按 label 查找歧义时返回候选供用户选择，不随意选择第一个。vectors 和内部整数 ID 由程序准备，普通清单不要求人工编造。

## 查询请求

```text
WaveQuery(snapshot, text="", vector=None, context=[], core_concepts=[], cues=[],
          candidate_filter=None, top_k=None, mode=None,
          include_trace=False, include_counterevidence=None,
          thinking_stages=[], candidate_pool=None)
ContextFragment(key, role, sequence, content=None, text=None, weight=1, time=None)
QueryCue(label, description, is_core=False)
ThinkingStage(name, candidates=[candidate_key], top_k)
```

ContextFragment 的 content/text 二选一；序位允许不同角色共享同一轮次，排序用 sequence 然后输入次序。时间只作明确记录/查询条件，不默认参与序位衰减。用户显式配置基于时间的策略另需可比较域当前点，本轮不隐式转换。

candidate_filter 可包含 candidate_keys 与按现行时间值定义的条件，AND 条件作用于同一事实组。Namespace/权限 owner 尚未接入，接口不宣称提供完整授权隔离。证据 scope 从构建清单确定，严格缩小需要新证据视图。

ThinkingStage 的候选均先与本次允许集合相交；不存在 key 是输入错误，合法但不满足 closure/真实路径的候选可不返回。无完整 transitions 时产生明确 thinking.unavailable 及原因，普通检索结果可继续交付。

## 查询结果

```text
WaveResult:
  snapshot_ref
  mode, used_lanes, skipped_lanes_with_reason
  hits: ordered WaveHit[]
  context_summary, cue_summary
  algorithm_versions, scope, coverage
  thinking: stages/routes | unavailable(reason)
  diagnostics?  # include_trace / explain 时展开
```

WaveHit 的 rank 决定呈现次序；quality/ranking/score_kind 见 ranking。命中返回精确候选及实际内容选区，来源和反证可以进一步展开。完成没有候选时返回真实空集合与原因，不制造默认答案。

coverage 包含候选预算、返回上限、实际候选数量及预算或 top_k 是否触发截断。

## 不可变与错误

配置、模板、snapshot、空间和输入在根操作固定。Native 不以外部可变 dict/ndarray 作为可长期持有的权威。所有非法空间、矩阵形状和行映射在构建/请求边界失败；成熟依赖的错误映射为共同 NousError，原始错误供诊断。

未知能力、无路径、缺向量、空结果、取消与存储失败分别表达。没有调用真实内核时不得返回 vcp-executed 标志。


## 可检索正文与媒体选区

Native 当前接收文字候选。纯文本按原 Artifact 的行/字节选区读取；PDF 由 MediaPreparer 在明确页选区上抽取文字。候选同时保存实际 text Artifact 的 content 与原 PDF 精确页选区 origin_content；多页合并结果保留各页有序 InputGroup，来源根仍指向原 PDF。

图片、音频和视频须先通过显式媒体解释/转写取得文字产物，再把产物 Artifact 作为候选。清单直接提供这些模态时返回 wave.text_required；无可提取文字的 PDF 页返回需要视觉处理的具体错误。

所有清单时间、上下文时间与过滤范围的 TimePoint 均经 TimeService.validate 校验已注册域。未注册域先报错，再进行同域比较。

## 固定候选与既有向量

VectorRef 包含已登记空间定义、purpose=query 和实际向量 Artifact/row；外部向量的高级进程内调用仍提供同一空间合同。选择 vector 时不再次用 text 计算 raw query；text 仍提供真实字面线索，上下文可按显式选项继续融合。请求内多个 raw 向量来源冲突时返回明确错误。

candidate_pool 是 `{snapshot, entries:[{candidate, lanes:{lane:{score,rank}}}]}`，保存绑定当前快照的有序候选及 lane 分数/名次。传入时 observe 后直接 readout，跳过内部 recall；allowed 过滤仍执行。普通 query 不提供它时使用同一个核心的 recall。

CLI 增加高级选项 `--vector-ref FILE`、`--candidate-pool FILE`，文件只是上述公开值的 JSON；Python 应用解析正式短引用并映射 Native 行号。用户不手写本地整数 row，不在 CLI 直接调用 PyO3。Thinking 与同次观测共同执行，不增加跨 CLI 命令的观测缓存服务。

多路径命中返回 `chosen_path` 的路径 key/label/精确 steps，并保留各路径的 B/P/G/A。原文读取仍使用候选的 ContentSelection。来源支持诊断将 `independent_support_fraction` 的实际比例与 `support_weight` 的数值衰减分开；未知比例为 null。

Python 的 `WaveSnapshots` 拥有 SQL、不可变目录、身份、Native 资源与已打开快照；`WaveQueries` 拥有 QueryPlan、时间/来源范围和结果物化。`WaveOperations` 在根 operation 内组合 prepare、Native 与精确内容回读。
