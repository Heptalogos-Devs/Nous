# 面向使用者的认知命令

## 正式入口

`note`、`journal`、`journalize`、`concept`、`query`、`read`、`feedback` 是长期保留的产品命令，调用共同 cognition 操作。`tools` 继续提供模型/媒体/内容/Wave 管理与实验能力。两者共享基础实现，低层工具不拥有另一个日记系统。

全部命令使用当前 `--instance`、`--json`、`--language` 和错误出口。`--namespace` 可覆盖默认域。正文通过 `--text`、`--file` 或 stdin 的明确单一来源读取；三者冲突时报参数错误。短引用使用实际返回值。

手工记录与 journalize 使用 `--source REF` 保存普通引用，`--current-basis REF` 明确依赖当前认识，`--inspiration REF` 保存启发材料。选中的来源按这三组依次分配 S1、S2 等局部键；来源在操作开始时固定精确版本。修订保留原来源；`--clear-sources` 明确移除，或重新传入来源替换绑定。

```sh
nous note add --text "今天发现取消等待后线程仍在读取文件。"
nous note add --text "调整资源持有范围后，关闭正常完成。" --remember
nous journalize --source note:... --source note:... --topic "取消与资源生命周期" --save
nous journal create --file ./review.md --title "资源释放的责任"
nous query "取消任务后文件仍被占用，有没有相关经验？"
nous read jrn:...@1 --sources
nous journal edit jrn:...
nous journal history jrn:...
nous journal annotate jrn:... --adopt
nous feedback jrn:...@2 --category accuracy --value negative --comment "第三段把线程结束写成了协程结束"
nous feedback jrn:...@2 --replace-file ./corrected.md --reason "更正线程与协程的区别"
nous query "资源应该何时释放？"
nous journal archive jrn:...@3
```

`journal edit` 打开当前正文，保存其 base revision；编辑器返回后提交 expected revision。冲突保留用户文件并显示当前版本与重新采用入口。直接 `journal revise --file ...` 要求精确目标或 `--expected`，不能悄悄先读最新版本替换调用者的基准。

## 草稿与标注

`journalize` 缺省返回草稿；`--save` 明确采用，`--target jrn:...@N` 修改指定基准。`journal draft read <draft-ref>`、`journal draft adopt <draft-ref>`、`--file` 编辑后采用使用同一操作。重复 adopt 返回原采用结果。`journal draft adopt --current-basis S1`、`--inspiration S2`、`--reference S3` 按已有局部键调整来源用途；`--target jrn:…@N` 明确重新选择采用基准，`--fork` 明确另建文稿。

`journal annotate --draft-only` 只产生提案；`--adopt` 按明确候选/新建策略采用。无需人工逐个批准概念。高级使用可 `--bindings bindings.json` 指定确切复用映射；普通用户只看到复用/新建的可读概念摘要。

`concept add/read/find/revise` 使用 name/description 与短引用。meaning 改变需要新 revision，更新旧 Journal 绑定须重新标注。普通 query 不要求用户提供概念 ID。

## 查询与精确读取

`query` 使用当前 namespace 视图并按需刷新；`--mode lexical` 不调用模型；`--snapshot wav:...` 固定旧视图。命中显示标题、最佳原文片段、精确 revision、简短来源和必要时间。`--explain` 展开实际通道/分数/概念路径；高级原始值留给 JSON，不把全部 Native diagnostics 默认倾倒。

`read jrn:...` 表示当前；`read jrn:...@N` 和命中精确引用保持固定。`--blocks B2,B3` 使用对应版本 TextView；不同版本的块键不得串用。旧版本读取显示“当前另有 @M”，不替换正文。

`feedback <精确引用>` 可评价当时的正文；`--selection` 接收命中返回的 ContentSelection JSON，`--snapshot` 与 `--candidate` 标识并核对某次命中，`--query-result` 则记录整次公开查询结果。明确更正使用 `--replace-file` 与 `--reason`。

`journal refresh` 刷新当前域配置 profile；`--mode lexical` 显式刷新词项。保存后的默认输出“已保存 @N；检索将在下次查询时更新”，不要求用户先理解 Native 快照。`--refresh` 在正文提交后接续当前 profile 刷新；`--refresh-mode lexical` 本身也触发刷新，并明确只构建词项视图。成功返回 ready；构建期间又有写入且产物未成为 current 时返回 pending。失败返回 saved=true、indexed=false、retrieval_state=failed、保存引用及具体错误/操作关联，不撤销正文。上述接续适用于手工记录和修订、草稿采用、journalize --save，以及显式 correction。

## 失败、流与 JSON

参数错误遵循统一 JSON 错误和非零状态；无法归类的异常显示实际失败摘要，--debug 展开堆栈。取消/超时保持共同异常上下文。业务保存成功但显式附加索引失败时，响应包含 saved=true/indexed=false 及 error，不把已提交内容说成丢失。

正文/机器结果走 stdout，进度与日志走 stderr。journalize 可以复用现有流，但 `saved/adopted` 只在相应事务提交后发出；已出现部分文本不标为正式 Journal。无须新增流协议层。

## 管理可见性

runtime check 汇总实际 Namespace 文稿数量、草稿/待复核数量、Concept、已发表 profile 代次、索引待更新及最近失败关联；统计来自各 owner。无需额外状态账本。日志以操作、对象标题、阶段、真实结果和耗时为主，ID 在需要定位时展示。
