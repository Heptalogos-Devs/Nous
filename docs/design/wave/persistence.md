# Wave 快照、索引发布与重开

## 两类保存

正式输入仍由 ArtifactStore/Embedding owner 保存。Wave 保存自己的集合清单、精确候选/Concept/来源映射与活动快照记录。Native 索引和几何文件位于 ResolvedPaths.index 下，是可重建资产。

`wave_collections` 保存当前指针，`wave_snapshots` 保存快照身份、owner、代次、profile、完整来源根映射和组件引用；快照直接持有所有根的 Artifact 引用。不可变投影单元保存每份文稿的候选布局、来源 roots、正文与候选向量引用；独立图单元保存共享的概念、关系、支持组及概念向量引用。快照成员保存单元引用，候选索引保存本次行号并支持按键批量读取。正式反馈和资产依赖使用关系表维护。

## 构建通路

1. 认知在一致读视图捕获正式版本和来源，或将手工清单接纳成 `CorpusView`。
2. `prepare_build(view, profile)` 在事务外按完整输入签名复用未改变的文稿单元及已采用向量，冻结 Native 数组。
3. 完整 Native 输入签名相同时引用已有不可变组件；输入变化时在稳定组件目录中完整构建。关闭构建期 reader 和 writer 后，Native 原子发布该目录的完成清单；Wave 只接纳通过清单与身份校验的组件。
4. Native 组件完成后，在短事务中登记未采用快照、共享单元、图、候选行及其正式引用。相同 build key 和完整输入签名可重放该准备结果；调用者获取轻量准备引用。
5. 在调用者的写事务中，`accept_in(tx, prepared)` 切换采用状态；认知调用者核验领域代次后在同一事务以 `promote_in(tx, ref)` 切换当前指针。未采用快照不进入普通查询、反馈或状态统计。

Windows 上 Tantivy 的 metadata 原子发布通过正式 Directory 接口接入 std::fs::rename，允许分享删除权限的旧读句柄继续读取原文件，同时新读者取得已提交 metadata；segment 文件、mmap、锁与格式沿用 MmapDirectory。

内容、模型与索引构建不持写事务。没有完成清单的目录是中断的构建，不可作为组件读取；已登记的准备与正式快照均持有组件和单元。准备事务成功后，同一 build key 从数据库恢复结果；事务之前中断留下的组件可重新使用或清理。相同 key 对应不同输入时报错。

## 重开与使用

`current`/`describe` 返回轻量 SnapshotDescriptor，不打开 Native 或加载候选。`describe_candidates(ref, keys)` 批量读取涉及的单元，保留请求顺序和重复键。查询按指定快照组装数据库中的行映射并范围化借用 Native 读者；Native 组件自行校验其身份与文件摘要。

正在执行的读取持有旧快照，发布和清理不会关闭其组件。`wave cleanup` 显式清理未保留旧快照及无引用资产；当前、在用、正在准备、显式保留及正式反馈依赖的快照继续保留。未采用的准备快照也保留其组件及所引用的数据库单元。清理先提交数据库删除；无引用组件的删除中断后由下次清理继续。

模型不可用时仍可读取、纯 lexical 查询；需要新的 query embedding 时才使用连接。已有合法 query vector 的进程内接口可离线执行，不以 dummy 向量顶替。

## 视图和范围

一个快照的证据范围在构建时固定。需要仅使用部分来源的推理视图时，以该子集和对应概念/关联重新构建几何与索引，可复用正式向量。不要只裁掉输出就宣称隐藏来源未影响背景。

candidate_filter 在固定证据视图内限制返回集合，普通的返回时间过滤可保留全快照背景。未来访问控制或历史知情 owner 可以提供新的构建子集；当前单操作者实例不预建 RBAC 系统。

## 前台操作与持久维护

直接 build/query 使用受管理操作。认知 refresh 在自己的短事务里核验捕获代次，再调用 Wave 的接纳与提升方法；build key 固定工作身份，重放恢复同源准备结果。领域编排属于 cognition.maintenance，见[持久工作](../foundation/durable-work.md)。

## 构建结果与重复加载

Native build 写完成态文件并返回 BuiltSnapshot 摘要。Wave 打开发布的组件核对身份，再登记数据库。反证邻接、Concept 倒排和范数缓存随组件打开准备。兼容基代可增量更新词法和向量索引；概念、事实、关联及构建选项不变时复用数学资产，具体规则见 [Native 增量构建](native.md#增量构建与旧代借用)。

查询文字和临时 cue 默认由共同 Embedding encode_query 编码，不因为每次查询都新增正式输入 Artifact。明确保存实验查询时再采用。活动引用仍固定源正文与空间。
