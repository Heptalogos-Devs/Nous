# NousQL 语言设计

当前通过 `nous query` 或绑定 Namespace 的 `CognitiveRetrieval.query(CognitiveQuery(...))` 提出自然语言问题，结果包含正式文稿修订、选区、来源及查询情境。使用方法见[认知命令](../interfaces/cognition-cli.md)和[当前检索](../cognition/current-retrieval.md)。

以下定义后续 NousQL 的语言目标：自然语言作为基础查询，选择器、时间、组合与历史表达式提供结构化控制。完整语言解析、HTTP/MCP、任意历史截面与模型重排按后续路线接入。已知人物、事项、对象或时间可以直接写入问题；未知指代保留原意。

```nousql
最近讨论过哪些发布风险？
她为什么改变这个决定？ --(指代消解)--> Alice 为什么改变了关于编译器设计的决定？
How does reader reclamation work?
```

语言入口将与程序内查询共用业务能力。当前 `CognitiveQuery.text` 提供问题，Namespace 句柄固定域身份，精确修订和来源由各 owner 核验；未来协议的身份及来源读取范围由各自正式合同确定。

## 选择对象

规划中的选择器可与普通文字组合。名称绑定到相应领域的正式目录，歧义应返回可选择的正式引用；Entity、Schema 与 Observation 目录待各域实现。

| 语法                                 | 含义                                         |
| ------------------------------------ | -------------------------------------------- |
| `#主题` 或 `#("较长的主题")`         | 增加语义文字                                 |
| `@entity("名称")`，简写 `@e("名称")` | 解析已有 Entity，召回实际参与关系            |
| `@entity("名称一","名称二")`         | 召回共同涉及这些 Entity 的对象               |
| `@tag("名称")`                       | 解析已有 Concept/Tag，直接召回实际成员及关联 |
| `@schema("标题")`                    | 解析已有 Schema 及其实际支撑目标             |
| `@object("宿主提供的对象引用")`      | 根据实际 Observation origin 查找来源和认知   |
| `@ref(正式引用)`                     | 精确读取该对象或修订                         |
| `@r(正式媒体引用)`                   | 把已有媒体资源作为召回线索                   |

`@ref` 和 `@r` 中的“正式引用”是语法占位记号。当前结果以 `RecordRevisionRef` 固定修订，`RecordRef` 表示稳定文稿；选段使用携带修订归属的 `RevisionSelection`。未来对象引用按本次查询的知识截点绑定 head；与 `$history` 同用时仍只绑定该截点的一项修订。

可读引用沿用[共同标识符格式](../foundation/application-boundaries.md)：Journal 使用 `jrn(...)`，Note 使用 `note(...)`，精确修订号置于括号外，如 `jrn(...)@3`；原件使用 `art(...)`。实际标识符从操作结果取得，带空格时整体引用。机器引用保留实例、域与稳定对象身份，内容范围使用 `ContentSelection`，不依显示字符串推断归属。

待接续的领域能力包括正式 Resource 名称目录、Observation/evidence 顶层返回和外部权威资源。当前可用参数由 `CognitiveQuery` 与 CLI 帮助定义。

单独的选择器也可查询。精确读取保持所选修订，直接使用权威数据；实体、标签、图式等选择器用于召回。读取名称和标签不会创建认知对象。

## 选择返回范围

```nousql
发布决策 $return(memory,schema) $limit(10)
相关原件与记忆 $return(memory,media)
讨论涉及哪些人物？ $return(entity)
```

规划的返回范围包括 `cognition`、`media` 与领域种类，例如 `concept`、`entity`、`episode`、`journal`、`schema`、`association`。具体种类和状态由各域正式模型定义。当前查询按文稿分组交付 Note/Journal 及已采用的概念标注；`top_k` 控制返回文稿数，`passages_per_record` 控制每篇交付片段数。

`$materialize` 展开所选对象的正式来源视图。展开仍检查来源读取范围，完整身份、实际来源事件和清除缺口保留在机器结果中。

## 查询时间与历史

| 时间轴     | 对应事实                 |
| ---------- | ------------------------ |
| `occurred` | 事件时间                 |
| `observed` | 主体收到材料的时间       |
| `formed`   | 认知或派生表示的形成时间 |
| `valid`    | 认识的适用区间           |
| `recorded` | 权威记录的实际时间       |

```nousql
发布事件 $time(occurred,within=30d)
最近收到的材料 $time(observed,within=7d)
发布记录 $time(occurred,from="2026-01-01T00:00:00Z",to="2026-02-01T00:00:00Z")
当时适用的规则 $time(valid,at="2026-01-15T00:00:00Z")
```

范围为起点包含、终点不包含；`at` 查询指定点。`within`、`at`、`from/to` 分别使用。未知时间无法满足已知时间条件。同一查询可组合不同时间轴，条件共同作用于同一个实际证据事实组。

```nousql
收到的事件 $time(occurred,from="2026-01-01T00:00:00Z") $time(observed,to="2026-02-01T00:00:00Z")
```

`$sort(occurred,desc)` 按实际事件时间降序排列；`asc` 为升序。其他时间轴使用相同语法。未知值排在已知值之后。

```nousql
$time(occurred,within=7d) $sort(occurred,desc) $mode(lexical)
```

当前默认时间域为 `utc`，单位为 Unix epoch 起的微秒。ISO 时间须带时区，自定义域保留明确坐标和精度。语言规划中可写 `domain="域名"`；相对时长单位为 `ms`、`s`、`m`、`h`、`d`、`w`，从本次实际 clock 点计算。

基础配置 `time.domains` 声明刻度比例：

```json
{
  "utc": { "kind": "utc", "tick_seconds": "1/1000000", "epoch": "Unix epoch" },
  "simulation": { "kind": "linear", "tick_seconds": "1", "epoch": "simulation start" },
  "frame": { "kind": "opaque" }
}
```

`utc` 单位固定。opaque 域保留域内坐标；相对现实时间换算需要线性比例。五轴坐标经 TimeService 核验已登记的域，媒体帧号、页码和相对时段各守其内容范围语义。

`$asof` 选择当时已经记录的权威视图，名称、成员关系和修订头都在该截面绑定：

```nousql
发布认识 $asof("2026-01-15T12:00:00Z")
发布认识 $asof(ago=30d)
```

`$history` 允许检索既有历史修订；可以与 `$asof` 组合。`$time` 筛选事实的时间，`$asof` 选择知识截面。

## 来源、偏好和探索

```nousql
发布决策 $source("文档")
发布决策 $exclude(source="草稿")
发布决策 $prefer("回滚") $avoid("未确认")
发布决策 $prefer(recent,recorded)
```

`$source` 匹配实际来源类型 `source_type`。更完整的机器条件在 `filters` 中：人物、会话 origin、来源类型、标签、模态、认知角色、形成方式及认识类别。组合条件遵循实际来源事实，SourceReadScope 继续对全部来源实施读取限制。

偏好只调整合法候选的顺序。`$explore` 明确启用受限联想，已有 Tag 的正式身份、实际成员和可用向量参与激活。

检索模式可用 `$mode(lexical)`、`$mode(dense)`、`$mode(vcp)`、`$mode(hybrid)`。语义模式使用配置的模型资源、共享向量和实际调用额度。`$effort(light)`、`$effort(normal)`、`$effort(deep)`、`$effort(maximum)` 分配候选工作量。

## 组合查询

普通文字中的 `&&`、`||` 和括号保留为文字。组合使用明确的 `$any` 或 `$all`：

```nousql
$any("发布风险 $return(memory)","发布经历 $return(episode)") $limit(10)
$all("发布风险", "回滚策略") $return(memory)
```

`$any` 合并分支结果，`$all` 取共同目标。分支继承根条件，局部返回范围进一步收窄。`$asof`、`$history`、`$limit`、`$effort`、`$diagnostics`、`$materialize` 位于根查询。分支可以选择自己的模式、时间条件和排序。

根级 `$sort` 在分支合并或取交集之后决定最终顺序，再按 `$limit` 截取。分支中的 `$sort` 仍决定该分支的顺序。结果的 `order` 保存实际时间轴和坐标；同域已知时间先排序，未知时间置尾，相同时间按相关性和正式引用确定顺序。

机器入口也接受同一 `expression` 树：`atom` 携带 `source`，`any` 和 `all` 携带 `children`。源文字与机器表达式编译为一个正式查询计划。

字面标记用 `\$`、`\@`、`\#`、`\\` 转义。把外部文字拼入查询时，通过 `QuerySyntax.canonical()` 或等价的字面转义保留其文字含义。

## 阅读结果

VCP 模式下，`@entity` 和 `@tag` 把已授权对象的精确身份作为本次 core，参与原生门控、传播、融合与直接接触评分。`#主题` 或 `#("完整查询概念")` 使用实际 embedding 生成临时查询向量；它们不登记新的 Concept、Entity 或关联。机器请求也可在 `vcp.core` 中给出精确认知引用，在 `vcp.concepts` 中提供 `{text,is_core}`。

`$explore` 请求 VCP 的 Thinking 方法路线，机器入口可设 `vcp.thinking=true`。`result.vcp` 保存实际绑定的 core、查询概念 producer 和 Thinking 阶段；路线使用正式对象引用。若当前图没有可用路线，所需能力为 required 时返回明确错误，optional 时保留 unavailable 结果与降级原因。`$diagnostics(none)` 保留这些请求结果。

优先阅读返回的内容与正式引用，再按需要展开来源。机器结果中的 `coverage` 说明本次覆盖状态，`plan` 包含实际绑定、截点、时间单位及分支工作量。`$diagnostics(none)` 省略计划；`full` 保留详细检索诊断。

计划预览只做解析与绑定，分支尚无 `stats`；执行查询后，`stats` 才给出实际候选数量、提供数量和截断状态。

查询会在配置的预算内执行。分支统计显示候选和实际提供的结果；有截断的交集通过覆盖状态表明尚有未完成范围。继续查询时复用实际返回的引用，遇到歧义从正式候选中选择。

## 可选模型重排

机器请求的 `rerank` 可取 `off`、`optional`、`required`；CLI 对应 `--rerank`。默认关闭。模型角色 `query_rerank` 使用 purpose 为 `rerank` 的 text-generation 路线及 `query-rerank` Prompt，输出候选评分。

重排在组合查询合并之后处理有实际文本投影的候选；显式根排序优先，required 重排与其冲突。请求预算限制提交的候选数量、每篇正文长度及总输入长度；模型调用账本保存实际输入。原生图片等没有文本投影的候选保持基础排序。

`optional` 在缺少模型资源、凭据或 Prompt 时返回基础排序，并在 `rerank` 中给出原因；实际提供方失败会返回错误。`required` 要求模型能力并传播失败。成功结果的 `rerank_score` 是模型给出的分数，调用账本保存实际输入、输出和用量。结果返回前重新检查来源与候选资格。
