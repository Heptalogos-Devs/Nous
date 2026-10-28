# 既有能力与源码采用

## 迁移依据

需求以 Architecture-Vault 当前《项目宪纲》和用户已定的 Python 主应用调整为准。`ses-nous-wave-imports-202610290007.md` 描述当前实际暂停点。源码和研究按本次引用的分支继续读取：旧纯Rust的目录形状可以重组，实际功能、语言、数值合同、来源与用户记录需要承接。

以下是开发索引，目标函数属于新设计。执行时应沿入口读到owner及数据库，而非只扫描README和符号名。

| 原位置 | 已有价值 | Python/native 的采用位置 |
|---|---|---|
| `crates/nous-vcp` | AGS/EPA/Pyramid/双场/RiverMemo/DTSC/Thinking与原数值研究 | `native/vcp` 直接改编，保留公式、配置及必要高价值案例 |
| `crates/nous-retrieval` | Tantivy中文tokenizer、USearch、检索代际 | `native/retrieval`，不迁成Python逐节点循环 |
| `crates/nous-domain/src/nousql` | 已实现语法与manual | 提取纯syntax到native；其余绑定/时间语义到Python |
| `crates/nous-app/src/retrieval/nousql` | bind/directives/execute/ranking、14类返回域 | `nous/retrieval`，共享ReadView、权限及真实领域读取 |
| `crates/nous-app/src/cognition` 与对应domain/storage | Memory及结构化认知、采纳组、修订、来源 | 对应 `nous/cognition/*` owner，SQL与规则一起重组 |
| `crates/nous-app/src/contexts` | catalog、prepare、admit、feedback、Steward | `nous/context`；保留真实模型输入与不同执行者视角 |
| `crates/nous-app/src/lifecycle/imports`、`nous-domain/src/imports` | Catalog/Unit/Output、输入归属、完成条件、文件去重WIP | `nous/imports`；按暂停交接逐项恢复实际行为 |
| `crates/nous-app/src/sources` | 来源预览、执行、trash/restore、认知撤回 | `nous/data_management/sources` 调所属认知owner |
| `crates/nous-app/src/bundles` / storage bundle | Native/A-M包、选择闭包、共享资源及来源重映射 | Python归档库+领域安装事务，保留公开格式与真实引用 |
| `crates/nous-foundation/src/configuration` | 注册、继承、active/desired、历史和通知 | Pydantic+配置service，清除重复父/子key |
| `crates/nous-foundation/src/models` | 资源图、路由、预算、账本、真实媒体parts | Pydantic AI/官方SDK，项目保留语义而不移植自制HTTP |
| `crates/nous-foundation/src/prompts` 与app prompts | 资源、激活、预览、真实模板 | Jinja2+现有文件及历史，逐份迁实际模板 |
| `crates/nous-foundation/src/media/content/platform` | 原件、区域、进程取消、FFmpeg采样、权限 | Python成熟文件/媒体/进程实现，保留来源与生命周期 |
| `crates/nous/src/{cli,http,mcp}` | 已有公开操作及用户参数 | Typer/FastAPI/MCP工具统一调用Pythonowner |
| `sdk/python/src/nous_wave` | 已有异步Client、managed、imports、files、sources、A-M调用 | 保留并类型化；禁止重新写一份同义SDK |
| `integrations/maibot` | 已做的真实工具/UI/文件传输接线 | 按新服务合同改动已有消费者，保留对用户有用能力 |

## A-Memorix 采用方式

阅读现有 `research/a-memorix-contracts.json`、上游 `A-Dawn/A_memorix` 与 MaiBot 中正在调用的版本。独立上游2.x仍在演化；稳定消费行为以真实工具、管理界面和协议源码为依据。

直接采用SDK、bundle格式读写、参数与结果模型、文本分块、时间/画像/图管理等可独立复用模块。把合适的完整实现复制改编到对应Nous owner，修正其依赖和数据入口；已经满足需求的代码不重写成新框架。

A-MemorixEngine 不能成为与Nous并列的第二个记忆数据库和任务中心。所需能力通过唯一Nous认知与事务模型提供，A-M协议层只处理真实外部参数/结果转换。对MaiBot、A-M bundle、公开RPC的支持是产品功能；旧内部Rust helper和私有RPC没有兼容保留要求。

完整覆盖工具：`search_memory`、`ingest_text`、`ingest_summary`、`get_person_profile`、`maintain_memory`、`memory_stats`，以及 graph/source/episode/profile/fact/runtime/import/bundle/tuning/v5/delete/correction/fuzzy_modify 管理。继续覆盖其真实mode、聊天/人物过滤、外部ID、返回错误、知识包、事实纠正、来源级联、整组恢复和gRPC按实际客户端需要的合同。不能以改一个工具名转发到简单query结束。

## 高级与独有能力续接

Memory-only Subject应有完整形成/查询/管理体验。Self、Social、Language、Motivation保持独有规则和按能力供给启用，避免用一个带几项字符串的通用KnowledgeBody抹平领域差异。

必须继续实现：

- Episode轨道、层级、成员与局部重分段；Journal写作时的知识视角；Schema支持、反例与lineage。
- Concept/Tag稳定身份、别名、合并拆分；AssociationEvidence的方向、极性和独立根；语义吸积、可访问性和真实使用反馈。
- Self的facet、NarrativeThread、DomainKnowledge、例外和多文档Seed；Seed按内容可进入Memory并保留设定来源。
- Social人物资料、跨平台身份对应、定向关系、信任领域、群体范围与LanguageConvention。
- Motivation的Desire/Concern/DesiredCondition、组合条件、achieve/cease/maintain/avoid、区间与unknown，不能替主体自动创建承诺。
- Active Cognition、ResidentSet、WorkContext、ManagedContext、Steward和持续认知维护；模型接触、真实使用、行为采用与结果反馈分别处理。
- 完整NousQL与全部认知返回域。Parser迁移保持现行manual，模型可读的短引用可继续定位对象；精确JSON保留完整身份。
- 多模态原件、区域、时间片段、完整组合顺序、三种解释策略、原生视觉向量、描述向量与图文联合检索。

当前语言迁移不重新缩小产品目标，也不在本轮把Hept世界/日历/主动行为搬进Nous。Hept通过正式Python客户端使用认知服务，MaiBot成熟平台、插件与管理体系继续作为其产品基础。

## 交接WIP的处理

文件接纳/来源/Bundle这批代码是设计素材与部分实现，不能先当作全绿基线。云端直接在新owner重建完整路径，无须为整套旧Rust程序重新编译成功才能迁移。

UnitOutput必须引用真实Import Observation；文件成功按实际Unit/Output配对保存；去重资格按scope/origin/policy/key/content和仍可用结果决定；恢复历史结果不能制造Job。Source撤回影响实际支持关系，剩余独立来源决定对象是否退役。无offset日期交给时间owner，不在导入调用者补猜测UTC。

这里的字段与规则应结合现有源文件调整为更清楚的数据模型，重要的是正式行为得到完整实现。缺少的行为在当前owner完成，不能通过额外caller回查、重组遗漏字段来弥补。

## 来源与许可

自有旧源码可以直接改编。第三方源码采用时保留已有LICENSE、NOTICE及必需的归属；不要给每个自有文件添加Agent元信息。当前VCP改编部分与旧独立Nous算法已有不同许可记录，按实际使用保存，发行前按选定方式处理真实分发义务。此事项不扩张为反复来源核验流程。

上游代码下载到 `temp/upstream` 或独立只读工作树，不提交整个第三方仓库、编译产物、凭据和无用fixture。真正采用的代码和必要研究材料进入正式owner；只用于阅读的目录留在忽略区。
