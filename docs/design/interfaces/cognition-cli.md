# 面向使用者的认知命令

## 正式入口

`note`、`journal`、`journalize`、`concept`、`query`、`read`、`feedback` 是长期保留的产品命令，调用共同 cognition 操作。`tools` 继续提供模型/媒体/内容/Wave 管理与实验能力。两者共享基础实现，低层工具不拥有另一个日记系统。

全部命令使用当前 `--instance` / `--profile`、`--json`、`--language` 和错误出口。可读实例 profile 见 [实例选择](instance-selection.md)；状态和未命名标题统一经 presentation 本地化，不持久保存中英双语标签。`--namespace` 可覆盖默认域。正文通过 `--text`、`--file` 或 stdin 的明确单一来源读取；三者冲突时报参数错误。短引用使用实际返回的完整标识符，命令行传入时保留引号。

手工记录与 journalize 使用 `--source "REF"` 保存普通引用，`--current-basis "REF"` 明确依赖当前认识，`--inspiration "REF"` 保存启发材料。选中的来源按这三组依次分配 S1、S2 等局部键；来源在操作开始时固定精确版本。修订保留原来源；`--clear-sources` 明确移除，或重新传入来源替换绑定。

```sh
nous note add --text "今天发现取消等待后线程仍在读取文件。"
nous note add --text "调整资源持有范围后，关闭正常完成。" --remember
nous journalize --source "note(...)" --source "note(...)" --topic "取消与资源生命周期" --save
nous journal create --file ./review.md --title "资源释放的责任"
nous query "取消任务后文件仍被占用，有没有相关经验？"
nous read "jrn(...)@1" --sources
nous journal edit "jrn(...)"
nous journal history "jrn(...)"
nous journal annotate "jrn(...)"
nous feedback record "jrn(...)@2" --category accuracy --value negative --comment "第三段把线程结束写成了协程结束"
nous journal replace "jrn(...)@2" --file ./corrected.md --intent correction --reason "更正线程与协程的区别"
nous query "资源应该何时释放？"
nous journal archive "jrn(...)@3"
```

`journal edit` 打开当前正文，保存其 base revision；编辑器返回后提交 expected revision。冲突保留用户文件并显示当前版本与重新采用入口。直接 `nous journal replace "<exact-ref>" --file ...` 要求精确基准；局部 patch 和语言 revise/rewrite/reflect 见 [维护入口](journal-maintenance-cli.md)。不能悄悄先读最新版本替换调用者的基准。

## 草稿与标注

`journalize` 缺省返回新篇草稿，`--save` 明确采用；不再接受 --target 或已知 Journal 的重复压缩输入。`journal draft read/adopt` 复用统一草稿能力；edit/rewrite 固定生成时的精确基准，不在采用时换 target。修改草稿正文和来源使用明确操作并保留原产物。重复 adopt 返回原采用结果；明确 fork 只将完整正文另建一篇。

`journal annotate --draft-only` 只产生提案；普通 annotate 按明确候选/新建策略采用。普通 annotate 直接调用 `AnnotationOperations.annotate`，提案、采用和读取已有 proposal-result 在同一根 operation 内完成；`--historical` 由同一方法控制历史版本的激活。无需人工逐个批准概念。高级使用可 `--bindings bindings.json` 指定确切复用映射；普通用户只看到复用/新建的可读概念摘要。

`concept add/read/find/revise` 使用 name/description 与短引用。meaning 改变需要新 revision，更新旧 Journal 绑定须重新标注。普通 query 不要求用户提供概念 ID。

## 查询与精确读取

`query` 使用当前 namespace 视图并按需刷新；`--mode lexical` 不调用模型；`--snapshot "wav(...)"` 固定旧视图。命中显示标题、最佳原文片段、精确 revision、简短来源和必要时间。`--explain` 展开实际通道/分数/概念路径；高级原始值留给 JSON，不把全部 Native diagnostics 默认倾倒。

`nous read "jrn(...)"` 表示当前；`nous read "jrn(...)@N"` 和命中精确引用保持固定。`--blocks B2,B3` 使用对应版本 TextView；不同版本的块键不得串用。旧版本读取显示“当前另有 @M”，不替换正文。

`nous feedback record "<精确引用>"` 可评价当时的正文；`--selection` 接收命中返回的 ContentSelection JSON，`--snapshot` 与 `--candidate` 标识并核对某次命中，`--query-result` 则记录整次公开查询结果。明确更正通过 journal patch/replace/revise 并关联 --feedback；意见列表与处理结果由 feedback read/list/resolve 提供。

`journal refresh` 刷新当前域配置 profile；`--mode lexical` 显式刷新词项。保存时 `--refresh` 接续当前 profile 刷新，`--refresh-mode lexical` 明确只构建词项视图；`--annotate` 请求标注。`--background` 将请求的维护工作与正文一起登记并显示 Job ID，`--inline` 等待维护结果，二者省略时使用 `cognition.maintenance.execution`。后台工作和长期宿主的操作见 [Job 与宿主](jobs-and-host.md)。前台维护失败时保留已保存正文并返回 partial。手工记录和修订、草稿采用、journalize --save 及 correction 共用这些选择。

## 失败、流与 JSON

参数错误遵循统一 JSON 错误和非零状态；无法归类的异常显示实际失败摘要，--debug 展开堆栈。取消/超时保持共同异常上下文。业务保存成功但明确附加阶段失败时，响应和根诊断均为 partial，包含已保存引用及问题；共同 CLI 输出完整结果后以 partial 专用状态 3 结束，不把已提交内容说成丢失。脚本据已保存引用继续维护，不盲目重发 create。

正文/机器结果走 stdout，进度与日志走 stderr。journalize 可以复用现有流，但 `saved/adopted` 只在相应事务提交后发出；已出现部分文本不标为正式 Journal。无须新增流协议层。

## 管理可见性

runtime check 汇总实际 Namespace 文稿数量、草稿/待复核数量、Concept、已发表 profile 代次、索引待更新及最近失败关联；统计来自各 owner。无需额外状态账本。日志以操作、对象标题、阶段、真实结果和耗时为主，ID 在需要定位时展示。
