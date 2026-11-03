# Wave 公共值与处理结果

## 引用

WaveSnapshotRef 保存 instance_id、snapshot_id 和 wav 短地址。WaveCandidateRef 保存 snapshot_ref、candidate_key。解析引用由 Wave owner 完成；内容仍由 ArtifactRef 与 selector 读取。row_id 仅在该 NativeSnapshot 内有效，不成为长期用户地址。

## 构建请求

CollectionManifest 的 documents/concepts/source_sequences/candidates/associations 见 assets。input 对象必须且只包含 file、text 或 artifact 一个来源；已有 artifact 可带 selector。candidate.content 的 document 必须等于实际正文来源，candidate.document 用于曲线文稿分组；两者不同必须在 manifest 明确说明，不能自动用 evidence root 代替。

derived_from 只引用 documents key，构建读取 DAG 的终端来源。evidence_group 是集合内独立支持分组标识，不作为原件 ID。association.evidence 也是 documents key 列表，解析后取得其真实根集合；负 polarity 保留，不送进正传输矩阵。

Concept.key 与 candidate.key 分别唯一，重复 label 允许；core 必须按 key 精确指定，按 label 查找歧义时返回候选供用户选择，不随意选择第一个。vectors 和内部整数 ID 由程序准备，普通清单不要求人工编造。

## 查询请求

```text
WaveQuery(snapshot, text="", context=[], core_concepts=[], cues=[],
          candidate_filter=None, top_k=None, mode=None,
          include_trace=False, include_counterevidence=None,
          thinking_stages=[])
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
