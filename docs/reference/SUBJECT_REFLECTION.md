# 共享 Subject 反思

[返回接口参考](README.md)

Runtime 将已实际观察的 Experience 合并为 `subject_reflection` MaintenanceNeed。Observation 接收不执行模型；重复 request 不新增范围。记录的来源包含原作者、实际观察序列、Session 与 WorkContext。来源失效只产生元数据变化；没有新的实际证据时阻断反思，不生成空模型结果。当前实现支持 Memory、Self、SocialRelation 与 LanguageConvention，Motivation 尚未接入。

显式配置 `reflection.enabled_domains`（默认空）并具备对应 Subject capability 后，Host 才能通过既有 maintenance grant 授予机会、操作数、模型次数与耗时预算。`reflection.debounce_seconds` 合并连续输入；`source_limit`、`source_max_bytes`、`candidate_limit`、`candidate_max_bytes`、`max_actions` 约束输入和动作。模型使用唯一 `subject_reflection` role，操作为 `SubjectReflectionText`，沿既有 execution profile 与 fallback 策略执行。

准备阶段只接收已观察来源和可信 Observation `context.cognition_usage` 的 SourceReadScope。模型只获得本批 source keys、真实 Host EntityRef catalog、来源结构 scope 与有界 current exact heads；缺失来源许可不能扩大候选。部分文本显式标注 partial，不声称已读取完整来源。新 Subject 没有 Seed 时不生成默认人格。

一次结构化 generation 返回带 domain 的 typed actions。Self 复用显式 interpretSelf 的准备、编译与 apply；Memory、Social 与语言约定分别调用正式 owner。模型不能申请认知管理权限、构造实体、扩大 scope、purge 或自动覆盖冲突。模型结果与 producer 在应用之前进入既有 model-results；各 owner 使用独立短事务和真实 work/action identity。

旧执行只确认捕获的来源进度；执行期间新增 trigger 保留。keep 也保存可检查结果。部分失败保留原付费 proposal 和已成功动作，`reflection.repair` 只能以原 operationId、semanticDigest、repairId 以及人工复核的 exactHead/expectedEpoch 修复失败动作，不再请求模型。`operationsConsumed` 取正式 Runtime 完成结果。

Official Client `reflection.inspect`、CLI `reflection status` 提供有界 pending/recent 状态与 payload 遗漏信息，可见 blocked、kept、partial、failed。`reflection.repair` 与 CLI `reflection repair` 使用同一公开合同。迁移 `0009_subject_reflection.sql` 增量保留已有数据与付费结果。

验证入口：`corepack pnpm smoke:subject-reflection` 使用隔离真实 PostgreSQL 和确定性本机模型，覆盖合批、重放、旧 trigger、新来源、三 owner 单次生成、partial 与无模型 exact-head 修复；它不证明 QQ 自然学习。真实 QQ 后续使用还要求共享 Query/Projection 的 SourceRead 前置过滤完整闭合。
