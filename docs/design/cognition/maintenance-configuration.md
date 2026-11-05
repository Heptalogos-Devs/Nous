# 本轮配置与真实消费者

## 字段归属

配置定义放实际 owner，接入现有 NousConfig、目录说明、作用域、有效快照和修改生效机制。新增字段不得只出现在 TOML 示例而没有消费者。

| 字段 | 缺省及约束 | 实际消费者与生效 |
|---|---|---|
| cognition.edit.task | journal-edit，非空 | WritingOperations.propose_edit；下一根操作 |
| cognition.rewrite.task | journal-rewrite，非空 | WritingOperations.rewrite；下一根操作 |
| cognition.reflection.task | journal-reflect，非空 | WritingOperations.reflect；下一根操作 |
| cognition.annotation.on_save | 保持 false | RecordOperations 的 AfterWrite 默认选择；记录核心不读取它 |
| cognition.search.passages_per_record | 2，>=1 | 正式查询 ResultGrouping；下一查询，不重建图 |
| content.text_cache.max_bytes | 32 MiB，>=0，0关闭内存保留 | 共同正文缓存；资源策略按现有配置应用机制 |
| content.text_cache.persist | true | 已配置 cache 根中的纯结构缓存；下一操作 |

journalize adoption、模型/媒体技术限额、具体检索通道、正文分片目标和语义空间继续使用已有字段。不要新增货币预算或费用确认。集合分组所需 group rows 属于快照结构，实际改动重新构建；每次 top_k 和片段数是查询参数。

模板说明由 prompt.toml 和 TaskDefinition 拥有，实际模型资源仍由 tasks 配置引用；不在 cognition 定义 Provider 客户端或备用列表副本。

## 字段信息

每个字段都明确类型、默认值、单位、是否允许缺失/null、范围、作用域、说明和生效方式。普通用户不必调整的字段放 advanced；适用空间的危险配置继续使用已有绑定规则。

用量/时间等技术约束保持可配置。缓存 bytes 与文稿分片 bytes 是不同用途，说明不能混用。所有配置在一次根操作内使用同一快照。

## 去除无效字段

本轮扫描受影响任务/媒体配置的实际消费者。上一轮已迁移的规则不恢复。发现仅暴露却没有读取的参数时，直接移到实际 owner 并迁移消费者，或删除无用途字段，不留下“以后再启用”的默认开关。

新字段的 config describe 输出和真实行为属于同一功能完成条件；无需为每个简单字段创建独立测试。

分组召回新增 `wave.query.group_scan_limit`：默认 4096，正整数，单位为每个向量通道允许检查的不同候选行数，advanced、下一查询生效。类型与默认值由 Rust QueryControls 导出，Python 沿现有 Native Schema 接入；不在两侧分别手写。达到上限即返回 truncated 与实际已取得组数，不静默全量回退。它和 max_union_candidates（进入 VCP 读出的行上限）分别说明。

新 title=None 不是“写入后本地化字符串”。Artifact display_name 无用户标题时使用稳定的 `journal.md` / `note.md`，用户界面在 presentation 中按 kind 和语言给名称。逐个迁移 `title + '.md'`、按 title 排序与序列化的消费者，不为 null 添加中文数据默认值。
