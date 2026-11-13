# 面向使用者的认知命令

## 正式入口

`note`、`journal`、`journalize`、`concept`、`query`、`read`、`follow`、`arrange`、`work`、`feedback` 是长期保留的产品命令，调用共同 cognition 操作。`tools` 继续提供模型/媒体/内容/Wave 管理与实验能力。两者共享基础实现，低层工具不拥有另一个日记系统。

全部命令使用当前 `--instance` / `--profile`、`--json`、`--language` 和错误出口。可读实例 profile 见 [实例选择](instance-selection.md)；状态和未命名标题统一经 presentation 本地化，不持久保存中英双语标签。`--namespace` 可覆盖默认域。正文通过 `--text`、`--file` 或 stdin 的明确单一来源读取；三者冲突时报参数错误。短引用使用实际返回的完整标识符，命令行传入时保留引号。

手工记录与 journalize 使用 `--source "REF"` 保存普通引用，`--current-basis "REF"` 明确依赖当前认识，`--inspiration "REF"` 保存启发材料。选中的来源按这三组依次分配 S1、S2 等局部键；来源在操作开始时固定精确版本。修订省略来源时继承，重新传入时追加；`--clear-sources` 明确清空。程序内精确删除或替换使用 `SourceChanges`，并核对正文没有悬空引用。

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

`note edit` 和 `journal edit` 在释放读取 Runtime、启动编辑器之前，将正文与固定目标、原 expected 的会话描述写入实例 data 下的 `recovery/edits/`。编辑器直接打开该文件。锁占用、取消、配置错误、版本冲突或未确认保存的其他出口保留文件并给出恢复路径。

`nous journal edit --resume <会话.json>` 或 `nous note edit --resume <会话.json>` 继续提交保留文件，核对实例、稳定目标及原 expected，并沿用会话已保存的维护请求。恢复期间出现版本冲突，显示当前版本、原基准与差异并保稿。正式修订的 write_key 识别同一次提交；保存已完成而退出或呈现失败后再次恢复不会重复创建修订。已确认保存的 partial 同样可以清理会话。没有修改的会话正常结束时清理。

直接 `nous journal replace "<exact-ref>" --file <保留稿.md>` 使用明确精确基准；局部 patch 和语言 revise/rewrite/reflect 见 [维护入口](journal-maintenance-cli.md)。恢复不会读取最新 head 充当原基准。

## 草稿与标注

`journalize` 缺省返回新篇草稿，`--save` 明确采用；edit/rewrite 固定生成时的精确基准。`journal draft read` 读取草稿，`journal draft revise --file` 创建修改后的新草稿，`journal draft adopt` 采用完整草稿。已保存成果可用 `journal draft from-result` 发起本地组装；重复采用返回原结果。

`journal annotate --draft-only` 只产生提案；普通 annotate 按明确候选/新建策略采用。普通 annotate 直接调用 `AnnotationOperations.annotate`，提案、采用和读取已有 proposal-result 在同一根 operation 内完成；`--historical` 由同一方法控制历史版本的激活。无需人工逐个批准概念。高级使用可 `--bindings bindings.json` 指定确切复用映射；普通用户只看到复用/新建的可读概念摘要。

`note annotate "note(...)@N" --background` 和 `journal annotate "jrn(...)@N" --background` 为当前修订发起本运行期后台标注，返回 operation ID；`--task` 可选择任务，`--refresh` 请求随后刷新。不参与召回的 Note 也可明确标注。人工提案、指定候选或绑定、`--draft-only` 和 `--historical` 使用前台入口。

`concept add/read/find/revise` 使用 name/description 与短引用。meaning 改变需要新 revision，更新旧 Journal 绑定须重新标注。普通 query 不要求用户提供概念 ID。

## 查询与精确读取

`query` 使用当前 Namespace 视图并按需刷新，返回分组材料及精确原文。配置决定使用的 Wave 程序；开发比较通过 `tools wave` 指定程序。需要在其他进程继续读取某次交付时使用 `--save`，返回的结果引用和 E 键一起标识片段。

也可以先将普通查询的 JSON 结果保存在文件中，决定使用后保存同一交付：

```sh
nous --json query "取消任务后如何释放资源？" > query.json
nous save-evidence query.json
```

命令返回结果引用，供后续读取、反馈和工作记录使用。

```sh
nous query "取消任务后如何释放资源？" --save
nous read "res(实际结果引用)" --part E1
nous follow "jrn(实际文稿引用)@2" --relation current_basis
nous query "这些决定现在仍适用吗？" --work "work(实际工作引用)"
nous arrange "res(实际结果引用)" --purpose "列出结论及适用条件"
```

`--steward optional` 请求有来源的阅读整理，整理不可用时保留已保存的原文和问题；`--steward required` 要求完成整理。整理结果和原文证据分别保存。用 `--budget 1000 --budget-unit characters` 或 `--budget 3000 --budget-unit bytes` 指定原文预算；`--explain` 展开诊断。已找回的材料会保留在结果目录中；预算不足或未选出正文时，结果显示原因并提供阅读材料的入口。工作使用记录可引用本次 E 片段，也可引用随后阅读的正式来源选区。

通过 `--work` 提供的目标、焦点、有效决定和活动依据默认参与召回及原文定位。需要调整自动情境的阶段时，可重复指定 `--context-stage retrieval`、`--context-stage localization` 或 `--context-stage evaluation`。

JSON 输出可用全局 `--select` 选择实际字段，支持重复指定标准 JSON Pointer。指针与值组成返回对象；不指定时返回完整 JSON。比如读取已保存的 E 片段正文：

```shell
nous --json --select /selections/0/body_text read 'res(实际交付引用)' --part E1
```

`read` 接受正式材料引用及其精确修订；`--blocks B2,B3` 读取同一材料版本中的指定块，`--sources` 展开所保存的来源。`read "res(...)" --part E1` 的键只属于指定交付。查询时用 `--prior "res(...)"` 引入先前交付，增加 `--anchored` 则在先前快照中查询；也可通过 `--snapshot` 指定快照。

## 持续工作与反馈

`work create` 创建目标，`work read` 读取当前或精确修订。结构化状态使用 JSON，说明正文使用 Markdown。修改时以精确修订或 `--expected` 指定基准，冲突保留当前修订。

```sh
nous work create "资源关闭研究" "整理取消和关闭的依据"
nous work read "work(实际工作引用)"
nous work revise "work(实际工作引用)@1" ./work-state.json --body ./work.md
nous work set-active "work(实际工作引用)@2" ./selections.json
nous work record-use "work(实际工作引用)@3" "res(实际结果引用)" ./used-selections.json "形成关闭顺序的决定"
nous work uses "work(实际工作引用)"
nous feedback record --evidence "res(实际结果引用)" --passage E1 --category retrieval --value positive
```

`selections.json` 和 `used-selections.json` 是 `MaterialSelection` 数组；工作使用记录保存实际采用的正式材料选区，也可记录查询后继续读取的来源。查询反馈的 E 键指向这次交付的片段。`feedback record "jrn(...)@2"` 评价精确文稿，`--selection` 接收选区 JSON。反馈列表及处理结果由 `feedback read/list/resolve` 提供。

`journal refresh` 使用实例当前配置刷新视图；保存时 `--refresh` 请求刷新，`--annotate` 请求标注。`--background` 在正文提交后启动本运行期工作，默认等待维护结果。后台工作见[宿主与运行期操作](jobs-and-host.md)。

## 失败、流与 JSON

参数错误遵循统一 JSON 错误和非零状态；无法归类的异常显示实际失败摘要，--debug 展开堆栈。取消/超时保持共同异常上下文。业务保存成功但明确附加阶段失败时，响应和根诊断均为 partial，包含已保存引用及问题；共同 CLI 输出完整结果后以 partial 专用状态 3 结束，不把已提交内容说成丢失。脚本据已保存引用继续维护，不盲目重发 create。

正文/机器结果走 stdout，进度与日志走 stderr。journalize 可以复用现有流，但 `saved/adopted` 只在相应事务提交后发出；已出现部分文本不标为正式 Journal。无须新增流协议层。

## 管理可见性

runtime check 汇总实际 Namespace 文稿数量、草稿/待复核数量、Concept、已发表 profile 代次、索引待更新及最近失败关联；统计来自各 owner。无需额外状态账本。日志以操作、对象标题、阶段、真实结果和耗时为主，ID 在需要定位时展示。
