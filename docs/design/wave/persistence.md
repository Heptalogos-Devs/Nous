# Wave 快照、索引发布与重开

## 两类保存

正式输入仍由 ArtifactStore/Embedding owner 保存。Wave 保存自己的集合清单、精确候选/Concept/来源映射与活动快照记录。Native 索引和几何文件位于 ResolvedPaths.index 下，是可重建资产。

数据库初期只需 `wave_collections`（名称及当前快照）与 `wave_snapshots`（id/identifier、manifest Artifact、space、build profile、实际版本、index location）。引用哪些输入 Artifact/向量结果通过正式引用关系维护；不复制正文和整个模型调用账本。

没有外部兼容要求，不创建 schema v1/v2 桥接。版本不支持时返回需重新构建；只重置授权开发实例。

## 构建通路

1. 接纳并固定清单里的文稿与 Concept 描述；解析源血缘、选区和时间。
2. 通过 EmbeddingOperations.ensure 批量取得实际向量和逐行映射；精确复用已采用向量。
3. 准备 FrozenAssets，Native 在 index 的本轮 staging 目录构建全部声明通道与数值资产。
4. 构建成功后同文件系统完成发布为不可变 snapshot 目录；打开得到的 snapshot 检查必要身份和结构。
5. 在短 WriteTx 保存快照与引用，并切换 collection.current_snapshot。数据库提交前不向用户返回正式标识符。

内容/模型与索引构建不持写事务。构建失败保留旧 current；已产生且未绑定的目录可在本次操作清理。文件发布成功而 SQL 失败不自动删除当前其它快照，也不重发模型请求。

## 重开与使用

查询打开数据库保存的明确快照，核对 space、dimensions、配置和实际算法版本。Native 文件 manifest 保存必要的完整性与数组/索引映射；不在每次 query 重新 hash 全库。

正在执行的查询持有旧 Arc；新构建可发布新快照，旧查询继续原内容。第一版一个 collection 的 build 串行，查询读取不可变版本；无需蓝绿部署或跨主机发布平台。

模型不可用时仍可读取、纯 lexical 查询；需要新的 query embedding 时才使用连接。已有合法 query vector 的进程内接口可离线执行，不以 dummy 向量顶替。

## 视图和范围

一个快照的证据范围在构建时固定。需要仅使用部分来源的推理视图时，以该子集和对应概念/关联重新构建几何与索引，可复用正式向量。不要只裁掉输出就宣称隐藏来源未影响背景。

candidate_filter 在固定证据视图内限制返回集合，普通的返回时间过滤可保留全快照背景。未来访问控制或历史知情 owner 可以提供新的构建子集；当前单操作者实例不预建 RBAC 系统。

## 暂不接入 DBOS

本轮 build/query 使用前台受管理操作和明确结果，长调用可显示进度/取消。DBOS 留给首次要求跨进程恢复的批量建索引/记忆整理。届时按既有事务内入队合同接入当前 build 单元，不在 Native 内新增任务表或重试调度。

## 构建结果与重复加载

Native build 只写完成态文件并返回 BuiltSnapshot 摘要，不先 load 全资产。Python 发布目录后 open 一次；成功核对后短事务登记。读者始终持有不可变快照。反证邻接、Concept 倒排和范数缓存随快照构建/打开准备，不每次 query 扫全库。

查询文字和临时 cue 默认由共同 Embedding encode_query 编码，不因为每次查询都新增正式输入 Artifact。明确保存实验查询时再采用。活动引用仍固定源正文与空间。
