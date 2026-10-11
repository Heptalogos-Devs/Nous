# Nous Wave 当前状态

[返回文档目录](../INDEX.md)

## 已提供能力

- **Memory-only Subject**：Subject identity 与 capability；Memory、CognitiveSchema、Episode、Journal、Tag、AssociationEvidence 的 Authority、不可变修订、provenance、history 和 owner 生命周期；前四类共用对象 / 修订 header、读取与生命周期入口，保留各自内容结构。
- **材料来源链**：Artifact、Observation、SourceRegion、DerivedRepresentation 与派生 DAG；Material owner 维护内容身份、来源区域、五轴时间和精确 lineage。
- **认知 Runtime**：Session、ResidentSet、WorkContext、UseEvent、Experience 与 MaintenanceNeed；Runtime 捕获交互经历，Memory owner 提供 Episode / Journal 与有界维护写入。
- **查询与上下文**：自然语言 Query、exact/history 读取、lexical/dense/runtime/concept/topology lanes、固定计划与 owner 最终校验；QueryRun 支持外部能力往返。Projection 在完成查询后单独读取 owner 内容并保留命中修订；Memory（含 Episode、Journal、Schema）与 Material 使用有界 grouped owner reads。
- **配置与模型调用**：统一配置 catalog、active/desired/effect 读取；Core 以类型化模型结果调用 canonical owner，owner receipt 与 Authority mutation 原子提交。已提交的 DerivedRepresentation 与 Serving 材料仍由各自 owner/cache 保存。
- **官方 Client / CLI**：正式 protobuf-js 消息与显式 ProtoJSON 边界；覆盖已有功能的 typed 命令、资源/材料/认知入口、分页读取、有限本地 request/result cache、显式手动 retry 和 shared MCP。模型请求不自动崩溃续接；重试可再次调用模型或读取外部资源。
- **可重建 Serving 与生命周期**：按实际 builder 与构建策略管理代次，查询 readout policy 不改变资产身份；向量材料按有效 source reference 绑定，purge 协调 Authority、Serving 与活动 Core context/query cache。
- **Windows x64 source-less portable package**：包含 Kernel 与所需私有运行时的装配和安装路径。

## 实验能力与质量边界

Associative topology 是显式、默认关闭的检索 lane；Native 与 VCP/reference/adapter 路径保留在研究与可选配置范围。算法与模型的检索质量不由数据合同或 deterministic behavior 保证，相关实测范围见[研究观测](../research/observations.md)。

## 当前限制

- 当前正式能力组合是 Memory-only。Self、Social Cognition、Motivation 与 Heptalogos integration 尚未提供。
- External Resource provider 由 Host 显式注入；默认部署不包含通用本地目录搜索器。
- 一个 Authority PostgreSQL database 和一个 Serving root 由一个 Kernel 实例独占；多个 Client 可连接同一 Core，但当前不支持多个 Kernel 共享该库或 Serving root。
- 旧 embedding 的来源关系需通过离线复制数据库与对象目录，再运行显式 ownership upgrade 转换；普通 Kernel 启动不自动猜测来源或迁移。无法归属的旧向量留在独立 backup table，不进入活动缓存，也不受 ordinary owner purge 删除；该备份有单独的保留和删除范围。参见[离线副本升级步骤](../reference/LEGACY_EMBEDDING_COPY_UPGRADE.md)。
- source-less portable 目前只提供 Windows x64；Linux package 未提供。
