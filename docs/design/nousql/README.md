# NousQL：自然语言与精确控制

自然语言是查询主体。`nous query` 和 SDK 的 `QueryRequest` 共用相同解析、绑定与执行；程序内也可直接提供 `QueryExpression`。普通括号、`&&` 和 `||` 保留为文字。

```nousql
当时为什么决定先等待资源退出？
取消和资源关闭 $limit(5)
$any("发布风险", "恢复成本") $limit(10)
$all("资源关闭", "取消") $sort(recorded,desc)
```

`$any` 合并分支，`$all` 按正式材料身份取交集；根条件约束各分支，分支条件继续收窄。Wave 在同一快照内取得和组合候选，按共同程序排序，并对合并结果只执行一次原文定位与总预算控制。覆盖情况分别记录各分支和整次交付。

## 选择对象

| 写法 | 作用 |
| --- | --- |
| `@ref(正式引用)` | 绑定精确材料；单独使用时直接读取 |
| `@r(原件引用)` | 选择引用该原件的材料 |
| `@tag("名称")` | 绑定现有 Concept；歧义返回候选 |
| `#主题` 或 `#("完整主题")` | 将主题作为查询文字 |

材料引用来自操作结果，包含 Note、Journal、Work 的精确修订、Artifact 和 Result 的命名输出。结果输出可写 `res(...):output`。带空格的完整引用在命令行中整体引用。Entity、Schema 和 Observation 的选择器等待各自领域目录，绑定时返回明确能力状态。

`$return(note,journal,work)` 限定当前支持的材料种类，`$return(cognition)` 包含这些认知材料。`$limit(N)` 控制最终材料组数。`$prefer("文字")` 与 `$avoid("文字")` 表达软偏好，`$explore` 请求正式关系与受限探索，`$diagnostics` 展开执行细节。

## 时间与顺序

时间条件作用于 `occurred`、`observed`、`formed`、`valid` 或 `recorded`。多个条件必须在同一实际事实组上成立；默认排除时间未知的事实。

```nousql
发布事件 $time(occurred,within=30d)
记录 $time(recorded,from="2026-01-01T00:00:00Z",to="2026-02-01T00:00:00Z")
当时有效的决定 $time(valid,at="2026-01-15T00:00:00Z")
记录 $sort(recorded,desc)
```

范围左闭右开。`at`、`within` 和 `from`/`to` 三种形式择一；相对时长支持 `ms`、`s`、`m`、`h`、`d`、`w`。自定义时间域通过 `domain=` 指定并由 TimeService 校验；相对时间需要对应域的时钟。历史知识截面 `$asof` 及历史领域目录待正式领域实现，当前绑定返回能力不可用。

## 继续研究

`QueryRequest.context` 可以指定精确 Work 修订、情境片段或已保存的交付；`anchor` 指定材料选区、先前交付及当前或锚定视图。原问题保持独立查询信号，实际采用的情境及来源见 `QueryIntent`。

`--save` 保存 `EvidenceDelivery`；普通查询返回后也可用 `save-evidence` 采用此前输出的 JSON。后续用结果引用与 E 键读取原文。`read` 可展开完整材料、指定块和来源；`follow` 返回可继续使用的正式关系。`arrange` 使用已保存的证据生成面向新用途的阅读稿。

需要精简 JSON 时，在命令前使用全局 `--select` 指定 JSON Pointer，例如 `nous --json --select /evidence/passages query "资源关闭"`。每个所选路径及其值保留在返回对象中；完整结果与操作 Schema 仍可直接获取。

`\$`、`\@`、`\#` 和 `\\` 表示字面字符。算法程序与数值参数在实例配置和研发工具中选择，不要求 Agent 将其写入自然语言。
