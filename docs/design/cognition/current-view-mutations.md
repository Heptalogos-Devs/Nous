# 活动认知状态与当前视图代次

## generation 的含义

Namespace.content_generation 表示该域当前可召回内容及其证据视图的一次正式变化。它由拥有变化的领域操作在提交事务中推进；Wave 记录其发布快照覆盖的 generation。操作参数不能替代实际状态判断。

继续保留 lexical/semantic 的 profile 区分、精确旧快照和捕获 G 后不追赶的构建算法。current 查询至少包含开始之前已提交的保存，构建过程中新的提交由下一查询或后续刷新处理。

## 标注对象与活动指针分开

Annotation 是固定正文修订、Concept 意义版本、位置和路径的不可变结果。annotation_heads 是某一正文修订当前选用的标注。相同 adoption_key 可以复用 Annotation，但本次激活请求仍须执行指针前置条件检查和更新。

共同采用事务按如下顺序完成：

1. 核验目标修订、正文、parser profile、候选意义版本与提案来源。
2. 找到相同 adoption_key 或创建一次 Annotation。
3. 独立处理是否激活：核对 expected_active_annotation；没有变化则返回 unchanged，有变化则写 annotation_heads。
4. 若受影响正文是 record.current_revision，且 recall=true、archived=false，则推进 generation。
5. 若本次维护要求自动发布，将新 generation 的 refresh Job 在同一事务登记。

historical 仅表示允许作用于旧正文。调用者以 historical 指向实际当前正文并改变活动标注，仍必须推进 generation。调用参数 activate_for_current 不再参与“是否需要使当前快照过期”的事实判断，可改为明确的 require_current 前置条件。

相同提案先 adopt(activate=False)，后 adopt(activate=True)，必须复用同一 Annotation 并激活；再次激活已经活动的同一 Annotation 不额外推进代次。并发用户已换成另一个活动标注时，后台结果保留但不能覆盖它。

## 其他认知修改

文稿新建/修订、归档/恢复、remember 切换、当前有效绑定变化及明确 correction 对现行 current_basis 后代的影响，继续在同一事务中推进一次代次。普通评价、草稿、历史标注和不影响当前视图的未召回碎片不推进。

一个事务内有多个有效变化时先汇集“该域发生了实际当前变化”，最终只推进一次。可用一个小型事务内返回值/集合完成，不建立持久 dirty 表、事件日志或新的队列权威。每个 owner 返回自身真实变更结果；NamespaceOperations 负责递增，不能由 Wave 猜测。

归档不会抹去历史引用；correction 与现实新状态、后来的 reflection 保持不同语义。`needs_review` 仍按正式来源图派生，不批量写重复状态。

## 当前快照发布

构建采用一次 ReadTx 捕获 G 和各 owner 的批量只读值；CAS、向量、Native 运算在事务外；发布时短 WriteTx 检查该 profile 与 namespace generation 是否仍匹配。匹配才更新 current 指针。发现更晚代次时保留本次快照，返回 pending/superseded 等准确业务状态；不回滚文稿，不循环追新写入。

Job 的 SUCCESS 表示这次工作已按其合同结束。Job 结果必须另外表达 captured_generation、covered_generation 和 publication_state，防止把“任务已完成”呈现成“此刻检索绝对最新”。

## 修改位置与检查

`cognition/annotation/`、`maintenance/`、`scope.py`、`writing/records.py`、`basis.py`、`retrieval/{capture,projection,operations}.py` 与 `wave/snapshots.py`。标注采用返回真实变化，完整用例组合刷新入队；覆盖当前/historical、重复采用后激活、旧修订处理、归档恢复、明确依据更正、并发写入和旧快照读取，保留 Concept 独立意义修订语义。
