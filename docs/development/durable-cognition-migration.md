# 本轮源码与文档迁移地图

## 同一能力的生产者和消费者一起迁移

本表用于定位真实责任，不能只新建右侧文件而保留左侧旧路径。新增名称是恢复包的实施细化；私有文件可根据代码内聚性微调，责任和公开语义保持。

| 现有位置 | 修改与替换 | 同步范围 |
|---|---|---|
| `runtime/bootstrap.py` | 服务装配只缓存稳定域身份；加入 Job client/可选 worker；修订资源关闭次序 | `open_instance`、初始化、status/check、包示例 |
| `cognition/contracts.py`、`services.py` | NamespaceRef、RevisionSelection、公开结果扩展 | 全部认知入口、序列化、CLI |
| `cognition/scope.py` | 唯一名称/ID解析与事务内代次推进 | default_namespace、rename、bound services |
| `cognition/writing/operations.py` | target/expected保持、来源协作、同事务入队、前台维护复用 | create/replace/apply/membership/adopt |
| `cognition/writing/records.py` | 固定正文提交核心，返回真实变化与代次，采用幂等 | 草稿、反馈、状态修改 |
| `cognition/writing/reading.py` | 只负责精确读取与批量读取；来源准备分支移出 | 查询、草稿、写作、引用展开 |
| `cognition/writing/generation.py` | 统一来源能力、输入前校验、冻结配方、上移成功结果恢复边界 | 四种写作意图、人工覆盖 |
| `cognition/writing/drafts.py` | 采用已发布计算、稳定结果关联、恢复不重发模型 | draft read/adopt、重复采用、override |
| `cognition/sources.py`（新增） | 来源输入 → 规范绑定/RevisionSelection | RecordReader旧select_sources、_freeze、feedback |
| `cognition/annotations.py` | 提案/对象/活动指针分开；事实驱动generation；保留过期产物 | annotate、人工提案、后台采用 |
| `cognition/annotation_task.py` | 认知专用的语言合同 | models/tasks中旧ConceptAnnotationTask、task_catalog |
| `cognition/retrieval/*`、`basis.py` | 保留精确归属、批量捕获、依据失效、查询生产RevisionSelection | SourceBinding、QueryResult、current profile |
| `content/artifacts.py`、`text.py`、`text_cache.py`、`mappings.py` | 统一范围与结构、无损选区、已采用表示的显式坐标回映、缓存完整blocking边界 | 模型、Wave、反馈、草稿、编辑 |
| `content/preparation.py` → `application/preparation.py` | 跨内容与媒体的成批准备迁移 | wave/assets、模型输入、embedding |
| `content/results.py`、`schema.py` | 成功计算描述/业务采用键，避免重放重复结果 | Generation/Draft/Annotation维护 |
| `models/{input,operations,computation,tasks,executor,prompts}.py` | lossless策略、持久配方/产物、无业务任务名分支 | preview/probe/真实请求/备用路线 |
| `media/{preparation,audiovisual,documents}.py` | 复用共同范围、按格式内聚、受管子进程 | 图片/音频/视频/PDF路径 |
| `configuration/{metadata,definitions,service}.py` 及各owner配置 | 一次校验、深度不可变、配置字段注册 | 全部消费者使用已校验 ConfigSnapshot.values、序列化、动态Native配置 |
| `application/configuration.py` | 统一执行边界、保留保存/生效结果 | 在线/离线修改、Namespace select/rename |
| `runtime/execution.py`、`observability/*` | Runtime绑定、控制等待不占业务槽、真实完成信号 | 已有worker、DBOS回调、shutdown |
| `storage/database.py` | 读写显式事务控制与BEGIN IMMEDIATE，同步Job引擎工厂 | 所有WriteTx、初始化、JobClient |
| `wave/{operations,snapshots,schema,query}.py` | 轻量descriptor/批量candidate；发布重放身份 | Feedback、status/current/read |
| `jobs/`（新增） | DBOS公开接口适配、生命周期、桥接、状态控制 | Runtime、CLI与维护用例 |
| `cognition/maintenance/`（新增） | 两个业务工作及具名workflow编排 | AfterWrite、annotation activation、refresh |
| `transports/cli/*` | 参数到同一API，增加job命令与运行入口 | expected、background、语言、partial |
| `pyproject.toml`、依赖锁、相关typings | 固定DBOS3.4.0，核对既有Python/SQLAlchemy组合 | 不复制SDK私有类型与实现 |

## 现行文档同步

并入 patches/current-docs.patch 后继续按实际实现同步：

- `docs/ARCHITECTURE.md` 与 `src/nous/INDEX.md`：新的依赖方向、Job/maintenance负责位置、完整调用链；不把构想写成已经完成。
- `docs/CURRENT.md`：当前能力、有效决定、未完成项；施工结束后把本轮真实完成部分移入已有能力，保留尚未做的范围。
- `docs/design/cognition/{integration,maintenance-architecture,written-records,exact-editing,citations,writing-intents,draft-adoption,annotation-adoption,current-retrieval,feedback,query-use-context,configuration,maintenance-configuration}.md`：接口、来源、AfterWrite、活动状态、反馈形状同步。
- `docs/design/foundation/{application-boundaries,operation-ownership,configuration,configuration-integration,configuration-changes,execution,transactions,database,tracing,operation-outcomes,durable-work}.md`：共同owner、事务/生命周期、运行状态同步。
- `docs/design/content/{reading-and-preparation,selection-and-text}.md` 与 `docs/design/foundation/text-structure.md`：共同范围、无损原文、批量会话和结构缓存。
- `docs/design/models/{input-planning,task-contracts,execution,prompts,resources,durable-computation}.md`：实际调用、任务归属、validate/outputs 边界、冻结绑定与成功响应恢复。
- `docs/design/media/preparation.md`、`docs/design/wave/{corpus-preparation,persistence}.md`：确定性格式责任、批量候选准备、正式描述和持久 refresh 复用。
- `docs/design/interfaces/`：CLI/SDK 的真实参数与错误、Job控制范围、运行实例锁行为、各语言文案。
- `docs/design/nousql/README.md`：明确当前正式入口与未来语言设计，移除旧MemorySemantics/Fact/legacy SDK的当前实现宣称，保留有价值的语言目标与五轴历史语义。
- `docs/plans/cognitive-use-roadmap.md`：本轮持久维护落点以及WorkContext后续方向。

原始 `docs/sources/001/*` 不改写；已完成计划不伪装仍在施工。无需为每份新增讨论文件更新 CURRENT，也不额外建立永久项目台账。

## 退役实现与验证材料

旧接口专属测试、fixture、脚本、转发导出随迁移一并清理，有价值场景改用当前公开通路。用户独立检查中的场景进入相应完整通路回归；临时探针和一次性样本留在忽略的 temp 目录，不提交新的探针框架。

本轮修改合同可以直接迁移所有内部调用；不能为让旧测试原样通过而恢复已经删除的内部路径。真实持久Job和正式历史引用按各自明确生命周期处理。
