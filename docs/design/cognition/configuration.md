# 认知操作的配置与实际消费者

各字段进入现有 ConfigModel/setting、NousConfig、catalog/explain/update，具有类型、说明、范围、生效与作用域。共同配置服务继续负责覆盖和快照，认知代码不另读 TOML/环境变量。

| 字段 | 默认和约束 | 生效与消费者 |
|---|---|---|
| cognition.default_namespace | `default`，Namespace 名称或稳定 ID，非空且必须存在 | next operation，入口选择 |
| cognition.notes.default_recall | false，bool | next operation，record_note |
| cognition.journalize.task | `journal-draft`，已配置任务键 | next operation，journalize |
| cognition.journalize.adoption | `draft`，draft/save | task/call 可覆盖，采用方式 |
| cognition.annotation.task | `annotate-concepts` | next operation，标注提案 |
| cognition.annotation.on_save | false | next operation，显式保存后是否接续提案并采用 |
| cognition.annotation.known_concept_limit | 12，整数 >=0 | next operation，既有概念候选清单；0 不提供候选 |
| cognition.search.space | null 或已配置空间 | 新 projection profile，查询；不是修改已绑定空间 |
| cognition.search.default_mode | `auto` 或 lexical/dense/tagmemo/rivermemo/hybrid/dtsc | next query；auto 仅按 space 配置选择 profile |
| cognition.search.include_review | false | call 可覆盖，待复核内容呈现 |
| cognition.search.include_exploration | true | call 可覆盖，探索文稿显式标 purpose，不伪装为经历 |
| cognition.search.local_candidate_max_bytes | 4096，整数 >=256 | 新 projection，基于完整 TextView 块的局部候选规划 |
| cognition.list.page_size | 20，1..200 | call 可覆盖，列表 |

这些默认是起步设计值，可按实际使用调整。已有 Wave top_k、算法与索引参数继续归 Wave；不在 cognition 再复制。单块超出 local_candidate_max_bytes 时保留整块或只提供整文，报告覆盖；不得截断代码块并假称精确段落。

`annotation.on_save` 在正文提交后接续提案与采用，手工保存、草稿采用和 feedback correction 共用 WrittenRecords.complete_save。标注失败仍返回 saved=true、annotation=failed 与具体 annotation_error；正文与草稿继续可读。默认候选按 exact 正文出现的已有 label/aliases 发现，最多 known_concept_limit 项，候选固定其意义 revision；调用方可显式提供候选覆盖此选择。

默认保存不刷新索引；用户 `--refresh` 显式接续 current refresh。此处不新增 on_write/debounce/workflow 多套调度配置。模型、模板、生成参数和 fallback 都沿当前 task/provider/preset；不在 Journal 专题重复其默认值。

Namespace 本轮负责数据范围。配置作用域只使用当前 Configuration 真正支持的入口；调用时 override 走既有机制，不建独立 namespace JSON 设置库。将来需要持久域设置时在共同配置 owner 补齐，不能在 cognition 私有实现。

媒体默认策略及操作时显式选择由 MediaAnalysis 统一解释。
