# 内容对象、处理结果与来源

## 产品对象

本轮只建立两种面向使用者的不可变记录。

**Artifact** 是一份已接纳的文字或文件。保存内部 ID、可读地址、媒体类型、显示名、原始来源说明、接纳/创建时间；可选 producer_result_ref 从正式结果输出关系解析，正文通过 CAS 内容引用获取。

**ProcessingResult** 是一次已成功完成处理的结果。处理可以是本地转换、模型生成、转写或向量生成。保存操作/任务、实际输入及选区、输出 Artifact、参数、完成时间、producer，以及必要的中间结果引用。

同样字节共享物理 Blob，不合并不同接纳来源。没有新的内容变更时 Artifact 可复用；将本地文件再次作为新来源显式接纳时可创建新 Artifact。初版不把资料的路径当唯一身份。

这些对象提供输入和结果的完整操作，后续材料、观察与认知领域可以引用它们；当前不预建 Note/Journal/Memory 全家族表。

## 地址与精确读取

内部 ID 使用 UUID。产品地址使用 `art:` 和 `res:` 前缀，加随机分配的三组 CVCVC Proquint，数据库唯一约束处理碰撞。复用 legacy 中独立、正确的编码方法或成熟 Proquint 实现；不从 UUID 截断生成地址。

本轮地址在实例内解释，结果同时携带 instance_id。未来认知域命名空间在正式接入时迁移合同，本轮不为尚未创建的 Namespace 伪造默认认知身份。

地址绑定不可变目标。一次请求中的 `S1`、`S1:P2` 仅为模型输入局部键；ResultStore 保存其精确 Artifact/选区映射，不能拿临时序号作长期地址。字节 digest 留在机器详情，正常输出展示名称、正文、短地址和必要来源。

## 数据关系

需要正式保存的关系是：结果输入（按顺序）→ 输入 Artifact/选区；结果输出（按名称和顺序）→ 输出 Artifact；输出 Artifact → 生产该内容的结果；结果 → 它实际消费的描述或转换结果。

结果的输出关系是唯一写入位置，Artifact 的 producer 是它的反向读取视图；中间描述结果地址也从实际输入的生产关系取得。每条关系只保留一个维护位置，其他入口查询该关系。source map、coverage、producer 是明确的值合同，可以序列化保存；不建一张任意 JSON 状态总表，也不另建可独立修改的第二份关系图。

`ArtifactStore.ingest(input)` 完成 CAS 接纳和短事务登记。ResultStore 先在事务外 `prepare_outputs` 发布需要的字节，得到纯的准备结果；`adopt_in(tx, producer, inputs, prepared_outputs, ...)` 在调用者的 WriteTx 中登记结果、输出身份和引用，不自行 commit。普通生成的 `adopt` 完整操作可以组合这两个步骤并拥有自己的短事务。

Embedding 等跨 owner 采用使用 `prepare_outputs → Database.write → SpaceRegistry.bind_in + ResultStore.adopt_in + 向量关联`，所有正式变化一次提交。禁止在外层 WriteTx 内调用另开事务的 adopt。仅在完成提交后才返回成功引用。

可以先分配结果/输出内部 ID，在同一事务按外键依赖顺序插入结果头、输出记录及关系，无需额外持久准备状态。

数据库基础只提供连接与事务。Artifact、Result、Embedding 的表定义分别由负责模块维护，初始化组合明确收集这些 metadata。开发中的不兼容 schema 直接迁移受影响代码或按授权重建指定开发实例，无内部升级版本链。

## 请求、输出与生成出处

需要保留的模型结果包含实际请求的语义快照：有序角色、文本、媒体 Artifact/选区引用、已编译模板版本、输出 Schema、选定资源和有效生成参数。媒体二进制复用 CAS，不在每条结果中保存 base64 副本。秘密头、凭据不进入快照。

producer 区分 deterministic（工具与版本、转换参数）和 model（协议、请求模型名、服务报告模型名、声明部署版本、请求 ID、用量、结束原因、attempts）。未返回的值保持未知。网关的隐藏上游路由只能按它实际暴露的信息保存。

结果正文可以是 Markdown、JSON、转写文本或向量文件。结构化结果经过程序校验后保存，原始响应的必要部分可以作为对应输出 Artifact 保留。Trace 用于排查，正式结果不依赖 Trace 长期存活。

## 阶段成功与失败

ProcessingResult 仅表示已提交的成功阶段，不承担排队、running、重试或恢复状态。三种媒体策略由 MediaAnalysis 在内存中组织，返回 completed_result_refs 和最终结果。

描述成功后先保存它。第二阶段失败，错误返回此前已完成的结果地址；该描述仍可读、可在下一次显式使用。失败的 JSON 不被当作成功结构化结果；必要诊断位于本次受控诊断输出。

Ctrl+C 后保留此前已经提交的输入和阶段结果，停止新的模型尝试与采用。进程突然停止可能留下未引用 Blob，由显式回收处理；本轮不承诺自动恢复未完成操作，不借此引入调用回执平台。

## 时间

原件 received_at 是真实 UTC 接纳时间；派生 formed_at 是实际生成/转换完成时间，recorded_at 是提交时刻。源媒体 PTS、PDF 页码、文本行位是内容内位置，不写成 occurred 或 observed。外部事件时间未知时保持未知。

## 对外读取

`content read art:…` 展示内容；`result read res:… --sources` 展示结果与实际输入。二进制通过 export 取得。调用者读同一地址不触发新解析、模型调用或补建认知。
