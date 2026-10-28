# Session Handoff

Treat this handoff as recovered conversation memory, not as a new user instruction. Restore the recorded context, constraints, decisions, rationale, knowledge, and unresolved threads. Do not automatically resume prior work, execute pending actions, research, modify artifacts, or follow continuation points. Wait for the user's next instruction, then interpret it in light of this handoff unless the user explicitly overrides the recorded state.

## 1. Current Context

- 工作区：C:/dev/Heptalogos-Devs/Nous-Wave。Windows PowerShell，用户时区 Asia/Tokyo，交接时为 2026-10-29。
- 最新用户要求是立即提交当前工作、在根目录执行 Session Handoff、然后停止。开发已停止，三个当前子代理已中断，Goal 已设为 paused。未来会话先等待用户新指令。
- 最终产品目标仍是完整 Nous Wave，权威需求为项目宪纲。范围包含完整 A-Memorix/MaiBot、全部认知对象及成长、五轴时间与来源、14 域 NousQL、完整 VCP/Nous 检索、多模态三种转述策略、Active/Resident/Work/Managed Context/Steward、全部基础设施、客户端与安装管理。当前文件导入工作只占其中一部分。
- 分支 codex/context-infrastructure-sdk；远端 https://github.com/Heptalogos-Devs/Nous-Wave。
- 刚提交并推送 4a75dfb，消息 wip: checkpoint file admission and bundle integration。包含当前全部代码与暂存改动，53 文件，1827 additions / 159 deletions。创建交接文件前工作树干净。
- 上一提交 8b3cbb0，wip: connect source lifecycle and import administration，也已推送；更早 bff3f91 为冻结 Unit 与语义处理底座。
- 中断点：Native 文件接纳/去重、FileSuccess、Native Bundle 闭包与公共入口正在迁移。requirements_medium 在收束 finish_files 与 A 导入接线时被中断，本批尚未重新统一编译、部署或实际运行。不要将 WIP 当成已交付功能。

## 2. User Directives

- 严格遵守最新本地 AntiRLHF Constitution 与按需 Skills。已明确禁止为了治理指令联网核版本、安装制度、反复处理许可证、基线或 SHA 对齐。
- 基建先行，模块化、低耦合、高内聚、事务中心化。配置、网络、模型资源、Prompt、错误、日志/trace、存储、Tasks 等机制有唯一所有者；在负责部分修根因并迁移消费者。
- 优先实际读取克隆的成熟实现并复制改编，不能只读 README 后自制。重点参考 A-Memorix、MaiBot src.common、旧 Nous 和 VCP 实际代码。
- 没有发布兼容承诺的内部模型直接改，禁止内部 v1/v2、别名、桥接、双轨与 SQL 迁移链。当前真实开发数据需要保留时，采用 temp 下的一次性离线调整；不清空真实材料/调用/任务。
- 手写代码文件最多 600 行，按职责合理分目录。不要通过压缩代码或浅转发层规避可维护性。
- 不留开发期探针、临时测试、场景 DSL、合成重复语料，不为测试塑造生产接口。不做无目的深测；验证真实入口、算法关键边界、权限、事务、取消和恢复后继续功能。
- 所有新下载、缓存、编译、安装暂存、临时文件放 temp。运行 Python/Cargo/npm 前执行 . ./scripts/dev-env.ps1。
- Prompt 短、任务单一、独立可编辑；实际输入、目标 JSON Schema、输出与原始调用可查看。媒体保留自由描述、直接结构化、原媒体加自由描述后二次结构化三种策略。
- 用户之前授权正常提交推送、控制本机 Docker New API 与真实模型调用。凭据使用本地受保护文件，不复制到交接或代码。
- 用户指定当前三个子代理为 GPT-6.1 Sol、medium、标准速度，不能 Fast/Ultrafast 或自行换模型。最多 3 个子代理，优先复用；本次已全部停止。
- .codegraph/ 存在：理解/定位/编辑代码先用 codegraph_explore，明确路径查询最可靠。未覆盖或陈旧部分才用 rg/直接读。
- 最新停止要求覆盖此前“不用每阶段停下”。交接文件是记忆，不是继续执行的授权。

## 3. Intent & Decision Timeline

- **Initially:** 基于开发讨论、项目宪纲建设 Nous Wave 的统一基础设施和真实记忆链，要求成熟源码改编与事务中心化。
- **Then:** 扩展到 A-Memorix 生命周期、维护、导入、完整 VCP、图资产、多模态、Managed Context 与客户端；用户反复纠正功能混杂、内部兼容、低价值测试与隐藏 Prompt 合同。
- **Later:** 当前批次完成冻结 Unit、持久处理/时间子任务、实际模型与共享索引接线，并实际运行真实文本、向量、来源权限、Native 恢复和父取消。
- **Later:** 接来源批次 Trash/Restore、真实删除操作投影、preview/dry_run、A 导入管理；发现配置父子 key 冲突并在唯一配置所有者修正。该修正未重新部署验证。
- **Latest:** 文件成功接纳与去重迁移中，用户明确要求立即提交、根目录交接、停止。4a75dfb 已推送，Goal paused，三个当前代理 interrupted。

## 4. Decisions & Rationale

- **Decision:** Unit Catalog、InputSource、TextUnit、UnitOutput、文件完整成功事实独立于本机 Task。
  - **Why:** 来源和完成历史可以随 Native 包恢复，不能通过重建本地 Jobs 伪造执行。
  - **Alternatives rejected:** Task 状态镜像、按源名称猜成员、恢复时制造任务。
  - **Status:** confirmed。
  - **Source:** joint conclusion / evidence-driven。

- **Decision:** Unit 固定 UUID、原文 TextSpan、上下文区域和切分配置；retry 读原 UnitId，改切分创建带 parent_catalog_id 的新 Catalog。
  - **Why:** 修正旧实现 global/local chunk ordinal 混用与重分割漂移。
  - **Alternatives rejected:** 用当前 chunk index 重切或猜原范围。
  - **Status:** confirmed，底座已实际运行。

- **Decision:** UnitOutput.observation_id 为必填，指同一接纳事务实际创建的 Import Observation。
  - **Why:** 只有 memory_ids 时，Purge 后无法从某次输出证明其精确事件与 origin。
  - **Alternatives rejected:** 从同 material 任取 Import、Optional 内部兼容、新造事件。
  - **Status:** confirmed；最新代码已迁，现有开发 DB 尚未调整。

- **Decision:** 原文 Memory、语义 Entity/Association、输出事实和真实共享向量请求在一个接纳事务落地。共同 structured adopter 与 embedding owner 被复用。
  - **Why:** 避免 A facade 或 Unit worker 各自提交/复制形成与索引规则。
  - **Status:** confirmed，旧部署中已实际运行。

- **Decision:** parent_job_id 表示独占子任务所有权，dependencies 不表示所有权。
  - **Why:** 取消处理父任务应递归取消 owned unit/time，不能取消共享 embedding 工作。
  - **Status:** confirmed，真实 3 个子任务取消已验证。

- **Decision:** FileSuccess 是完整文件真实成功的历史事实；manifest head 只是当前资格指向。
  - **Why:** all-skip 可以只有接纳回执，无空 Catalog 或假 Job；跨重启/恢复也能保留完整成功历史。
  - **Status:** confirmed 设计，当前 App 接线尚未统一验证。
  - **Source:** joint conclusion。

- **Decision:** 去重按 namespace/subject/typed origin、policy、typed key 隔离；还要内容一致、精确产物当前 live 且 SourceRead 可读。
  - **Why:** 不能把 chat A 的完成当作 chat B 接触，不泄露不可读历史，也不能只按 source label 猜产物。
  - **Alternatives rejected:** facade 私有 manifest、仅比较路径/hash、用 worker 资格扩权。
  - **Status:** confirmed。
- **Decision:** server 将 file_identity 作为 opaque key；SDK 通过 ManagedFiles alias + relative path 生成。
  - **Why:** 不能把外部宿主路径按服务端 OS lower/resolve。成熟实现中的 lowercase absolute path 不跨机照搬。
  - **Status:** confirmed。
- **Decision:** ImportFileUnitOutput 每配对保存实际 vectors 策略，删除 Success 顶层单一 vectors。
  - **Why:** retry 可改变策略，完整文件可以由混合真实成功尝试贡献；不能谎称全部 Required。
  - **Status:** confirmed，已改 Domain；本批未编译。
- **Decision:** finish 只处理此次 unit_ids 涉及的 input_source；相关文件内可汇总准确 UnitId 的实际本机成功尝试，不局限祖先 retry 链。
  - **Why:** 解决并发 retry 完成次序导致漏记文件成功。须同 Tx 核 Job Succeeded、结果 output_id、checkpoint scope/unit/catalog/origin 和实际 Observation；无本地 Job 的历史 Output 不补造成功。
  - **Status:** confirmed 方案，requirements 被中断时仍在收束实现。
- **Decision:** Native 安装保留 FileSuccess 历史 origin；只有实际映射后的全部 Import Observation.origin 与 head.origin 相同才接受当前 head；merge 不覆盖已有本地 head。
  - **Why:** receiving placement 改变上下文时不能沿旧 origin 误判已导入。
  - **Status:** confirmed，storage 报已接；未实际运行。
- **Decision:** Source Trash 最后有效支持撤回后软退役当前 Entity，保身份/历史；有其他有效支持的共享 Entity 继续保留。
  - **Why:** 不能为通过正支持校验补回已撤回来源、让实体继续活跃。
  - **Status:** confirmed，代码已修；尚待新宿主实际验证。
- **Decision:** Source Restore 以原 Trash receipt.commit_seq 为 T，同批 CAS 恢复原生命周期及图谱快照；A DeleteOperation 的身份始终 T，状态由真实 Restore R 同快照关联派生。
  - **Why:** 不能按当前 label 猜撤销范围，也不能用事件 completed 冒充逻辑操作 restored。
  - **Status:** confirmed，入口/owner 已写，未实际运行。
- **Decision:** Source preview 与 execute 共用事务体，preview 最后 rollback，仅给原修订计划和实际预测计数，无 receipt/token/provider/wake。
  - **Why:** 真实 MaiBot UI 要求 preview，源目录统计不能代替完整授权/图谱计划。
  - **Status:** confirmed，代码已接，未实际运行。

## 5. Knowledge & Evidence

### Verified：已部署的 Unit 真实链

- 当前正式实例 temp/instances/codex-product-20261026，HTTP http://127.0.0.1:7441，安装程序 temp/installations/codex-product/nous.exe。
- 最后运行状态文件 run/host.json 记录 PID 13984。最新部署 stage 在停服务前失败，旧安装实例未被本次停止/替换。交接期间未停 Docker 或产品服务。
- namespace 7626d5a7-13d6-4eb3-85e4-abc611bc0b92。
- Codex subject dfdce2a6-0aca-44ee-8301-82e583282d39；MaiBot subject afc55ef6-7978-4ca2-8235-344681551ce5。
- 用真实 temp/开发讨论原文.txt 冻结 factual Catalog db0519f9-3bb2-459d-8701-881a74b5f425，共 5 Unit。
- 实际处理 Unit 60fde2b6-177e-4196-96b7-14b987b1696e；父 Job 01M619RR5GE1N5C092ZRHCFK1J，unit Job 01M619RR6ZJGKT1FJJ7W978Z1D。
- 语义调用 e35f82f4-09c9-448e-88fc-88678fe00798 成功，2247 provider tokens；保存 1 原文 Memory、62 Entity、35 Association。
- Memory 8e0531c2-bb0e-49f3-8c98-47a24e8f12cb；Output 3bc47917-81f7-4272-998e-b6c9fd3bfa9a；Import Observation 1cb06576-3375-4ff9-a237-0f20d731c9ce。
- 76 个真实向量 Jobs 全部 Succeeded；98 个向量关联复用 76 份内容。父处理 Succeeded。
- 单 SourceRegion 资格能读取对应 Memory exact content，读取整份 Original 被 Forbidden；没有反向整容器授权。
- Hybrid 查询真实 embedding call d00ef41c-b75e-48fc-9383-643e5857b3e2，旧查询快照中 114/114 eligible embedded、complete=true。
- 查询 domains 是 cognition/media，不是 memory；种类用 kinds 或 NousQL 选择。
- 新父取消 Job 01M61AHMX1EAJCB42T29JDGSKS，三个 owned children 全部 Cancelled，具体引用见 import-owned-cancellation.json。

### Verified：本批 Native 恢复

- 已在旧部署成功导出指定 processing task 的 Native 包，并恢复到新 subject 5a38f88c-d6cb-4f01-8fa7-c20136d56f8d。
- Installation 065dffea-6363-460d-873a-4957198a92b8。
- 包保留 1 InputSource、1 Catalog、1 selected Unit、1 Output、1 Memory、97 cognition、77 实际调用、76 vector contents；新主体 local Jobs=0。
- 原 Catalog original_unit_count=5，恢复 retained_unit_count=1，精确原文一致。
- 映射：Memory e54c90ad-b1b2-47e6-acc7-4d643751fd2f；Catalog 06e286e9-eb8a-49b0-893b-92ffcb61bbbe；Unit 8a5ac2d4-67a6-4c46-b1d1-9afb9e013361；Output a0bac181-6f99-42e5-a4b1-291d95aac39a。
- 恢复主体 Hybrid 查询 98/98 embedded，generation g-9245f8e6c4d87e964941eead，query call 2dc9940e-fb73-43fb-b686-aa8ccf2a8732。
- 此查询保存为“重启前”材料。本批随后部署失败，尚未做这组数据的新一次重启后比较。更早真实链已有重启恢复，但不能把它替代本批检查。

### Verified / Observed：模型与时间

- 本地 New API Docker 容器 new-api-local-new-api-1，127.0.0.1:3000，image nous-new-api:ark-multimodal；凭据只从实例 credentials/new-api.token 读取。
- describe 使用 doubao-seed-2.0-mini；native embedding 使用 doubao-embedding-vision，实际 provider model -251215、2048 维、normalization none；rerank qwen3.7-text-rerank。
- Codex subject 的 import_narrative/import_factual/import_time 已通过配置服务绑定 describe，每角色 8 calls/day。
- 为真实文件抽取向量调整 gateway 200/day、native model 与 embedding role 120/day；没有绕过预算服务。
- import_factual 的输入、Prompt 和目标 JSON Schema 已经正式 model_contract 读取，临时文件 import-factual-visible-contract.json。
- 可选时间调用 5d63d003-c87e-49b8-9012-8cf4f2103f6e 实际成功返回 JSON，但日期缺 offset，归一化保存 temporal_error。正文 Unit/parent 成功，原始结果可读，event_range 未伪填，Memory validity 不用事件范围代替。
- 当前时间归一化只接受明确 offset 的 RFC3339。日历日期精度与无时区表达是未完成的时间基础服务能力，不能在 caller 补 UTC 或机器当前时间。

### Observed：构建与部署状态

- 早先 source/import 快照 workspace Clippy -D warnings 与 cargo build 成功，日志 temp/runtime/source-import-integration-{clippy,build}.log。
- 该二进制执行 update stage 时启动失败：Conflict: overlapping configuration key: integrations.amemorix.imports。
- 根因已在代码改为唯一 AmemorixPolicy.imports 字段，删除父/子重复 ConfigKey/register，所有 settings/create 读取父快照的 imports；不是 Registry 豁免或内部兼容解析。
- 修正后的当前代码尚未重新统一编译。临时 build/debug/nous.exe 仍可能是启动失败的旧构建，不能当作最新可用程序。
- Python Ruff 在本批较早时通过；最新追加的 App/Domain/Native 代码未经统一 Cargo。无需继承“全绿”的结论。

## 6. State

### Resolved / Completed

- 4a75dfb 当前 WIP 已提交推送，8b3cbb0 与 bff3f91 也已在远端。
- 三个指定标准模型代理已建立并复用；用户停止后全部 interrupted。
- Goal paused。
- 已部署旧 Unit 底座的真实模型→Memory/Entity/Association→真实 embedding→检索→精确来源→Native 恢复，以及 owned 子任务取消，见上面具体证据。
- Source 生命周期、公用入口、A SourceAdmin/DeleteAdmin、preview/dry_run、逻辑 T/R 查询已经写入代码；这是实现状态，不是实际运行验证结论。
- 文件 Domain、Storage、Native Bundle 与入口已写入 WIP；不是完整产品完成。

### Current / Unresolved

- requirements_medium 的 admission.rs、processing/finish_files.rs、A imports 接线在中断时尚未报告 App 最终 ready；先核当前源码，不假定没有编译或语义缺口。
- File Domain：
  - AdmitImportFilesRequest 输入为 documents + optional opaque file_identity、切分/模型/时间/向量/origin/dedupe/force/clear。
  - ImportFileSuccess 保 input_source/catalog、file_identity、typed origin、完整 unit_id/output_id/vectors 配对、历史 processing_job_id、recorded/commit。
  - ImportManifestHead 按 scope/origin/policy/typed key 指向 success。
  - none 仍保真实成功事实、不维护 head；force 绕过 hit；clear 只清当前资格。
- Required 请求仅命中所有历史贡献 pair 均 Required 的完成事实。当前索引覆盖仍由 index owner 报告，不能把历史策略当作当前 coverage。
- SourceCore/A 管理新链尚未实际运行：需要区分 preview 回滚、默认保护/显式 force、最后来源实体退役、精确恢复 Frozen/pinned/保护期限、向量实际重挂和 T 状态 restored。
- A 其余管理动作、JSON/LPMM/temporal backfill/MaiBot migration、请求级 file/chunk concurrency 等仍有未完成能力。不得将明确 Unavailable 或局部接线算作全功能完成。
- 全产品目标仍包括完整认知成长、完整 NousQL/VCP、所有媒体及上下文、统一管理界面/CLI/SDK、三平台安装。docs/能力续接清单.md 保持开放范围。

### Blocked / Unknown

- 当前没有需用户补凭据的外部阻塞。
- 新 schema 与现有真实开发 DB 不相符，尚未离线调整，也尚未新部署。
- 新字段包括 Catalog.documents.original_unit_count、必填 UnitOutput.observation_id，以及新成功/head 表与 BundleResource kind。
- 交接前只读检查：2 Catalog，均单文档、原 count=5；原主体 retained=5、恢复主体 retained=1；4 Output；131 Task records；280 bundle_resources。
- temp/runtime/refresh-file-admission-schema.py 已准备但从未执行。它要求 fresh source-lifecycle-stopped.json 表明实例已停，备份 SQLite，然后按真实保留 Memory/Import 关系补 4 Output、补每文件原 count、重建 output/bundle_resources canonical DDL、新建 file 表，保留任务与资源并检查 FK。
- 此脚本仅为当前开发实例一次性保留，不是生产迁移框架；不能在服务运行中执行。根本没有新部署成功的断言。
- 较早生成的本地 Native 归档采用旧必填结构。其原字节仍保留；本批未提供旧归档转换，不承诺新 binary 能直接解码它们。新归档与新数据的完整链仍需真实验证。
- 没有运行中的本轮 Cargo/部署 exec 等待句柄需要恢复；最近 build/stage 均已结束。产品服务与 Docker 没有随开发停止而关掉。

## 7. Failed Attempts & Gotchas

- **Tried:** 只凭配置 key 存在判断 import role 已绑定。
  - **Result:** builtin key 的 json=null，Job 正确 Waiting configuration。
  - **Why:** 需看实际 binding 值；后通过 Config 绑定，原 Job 自动恢复。
  - **Retry?:** 已解决，不新造 Job。
- **Tried:** 默认 250k JSON nodes 导出含 76 向量内容/原始调用的 Native 包。
  - **Result:** invalid knowledge bundle；cause 为 JSON node/depth limit。
  - **Why:** 实际约 12.8MB JSON 超节点预算。实例配置改 2m 后原操作成功。
  - **Current code:** 默认 2m，foundation/json 精确 ResourceExhausted 分类，Bundle 保留其 code 和配置键；尚未在新宿主验证。
- **Tried:** 编译与子代理跨 crate 定义改动同时进行。
  - **Result:** SourceOperationRequest/SourceOperationsRequest 短暂 E0425。
  - **Why:** 同一次构建读到不一致编辑快照，不是缺 pub use。稳定后统一编译成功。
  - **Retry?:** 应等本批各 owner ready，避免循环编半成品。
- **Tried:** 新二进制 update stage。
  - **Result:** ConfigKey overlap，尚未进入 service stop。
  - **Why:** integrations.amemorix 父对象与 imports 子键重复注册。代码已修唯一 owner，未重建验证。
- **Observed:** 新 Output DDL 最初 FK(namespace_id,observation_id) 指向 observations(namespace_id,id)，不是 unique。
  - **Current code:** 已修 observation_id→observations(id)，writer 继续核 ns/sub/material/Import；不能新增无业务字段或重复 unique 凑 FK。
- **Observed:** Source 最后支持撤回后原实现补回 identity_provenance 但保持 active。
  - **Current code:** 已改软退役并返回真实 retired_entities，待实际运行。
- 不要在服务更新 exec 仍运行时请求 API；等同一会话终态。wait_jobs 超时不是 Job 失败或重启理由。
- SDK job 返回 {job,queue,model_calls}；wait_jobs 返回 Jobs 列表。SourceReadScope 字段是 source_refs/origin_conversation_refs。
- CLI stage 回执本身保存为 JSON；activate --file 指该回执，不是其中的路径属性。不要使用失败 stage 的错误 JSON 去 activate。
- 不把 generic async 巨大 match 放回同一个 Future：此前 Windows __chkstk 栈溢出已在 Operation/A facade 用普通 fn 选择 BoxFuture 分支修正。不要靠加大栈解决。
- 所有临时 Python 先加载 dev-env，防止 SDK 下生成 __pycache__。临时检查不得入库。

## 8. Open Threads

- **File admission / dedupe**
  - **State:** WIP，requirements 被中断。
  - **What remains:** App finish、并发 retry 合并、A 全 skip/实际 consumer 呈现、统一编译和真实材料运行。
  - **Continuation reference:** crates/nous-app/src/lifecycle/imports/admission.rs、processing/finish_files.rs；Domain imports/files.rs；Storage imports/files.rs。
- **Native file facts / selection**
  - **State:** storage 报 Domain/snapshot/remap/install/validation 已接，Root 已改 selection 闭包。
  - **What remains:** 全组编译、DB 保留式调整、新事实完整包往返、部分选择不扩散、Purge 后 Observation 追溯。
  - **Continuation reference:** bundle/files.rs、install_files.rs、selection*.rs。Root 的 selection 已显式选 output.observation_id；partial Unit 不自动带兄弟，只有全部配对已选才附成功事实，head 随 retained success。
- **Source lifecycle and MaiBot consumer**
  - **State:** code ready / live not verified。
  - **What remains:** 正式 SDK/真实 MaiBot UI 的 preview→execute→get/list→restore；计数、保护和原生命周期。
  - **Continuation reference:** App sources/、integrations/amemorix/sources/、SDK sources.py。
- **Config startup correction**
  - **State:** code fixed / not rebuilt。
  - **What remains:** 当前配置注册与启动真实确认，运行设置展示是否包含新 imports 字段实际默认值也尚未观察。
- **Time precision**
  - **State:** 已见真实无 offset 日期结果，原始输出/错误保留。
  - **What remains:** 在时间基础 owner 做日历日期/粒度和无时区表达，不能在导入点补猜测。
- **Full product**
  - **State:** 未完成，Goal paused。
  - **Continuation reference:** docs/首批开发任务.md 的 Objective/Decisions/Current Work、docs/能力续接清单.md、权威宪纲。以上均不是恢复后自动执行命令。

## 9. Relevant Resources

- 当前权威：docs/requirements/项目宪纲.md；用户最新 AntiRLHF/AGENTS 指令。
- 当前规划：docs/基建实施规划.md、docs/首批开发任务.md、docs/能力续接清单.md。
- 实际合同：research/a-memorix-contracts.json。
- 实际克隆：temp/repos/A_memorix、temp/repos/MaiBot、temp/repos/Nous-previous、temp/repos/VCPToolBox、temp/repos/rust-genai、temp/repos/apalis。
- 当前三个代理：
  - /root/requirements_medium：App imports/Domain 文件合同/A 导入；interrupted，最后 finish 未最终 ready。
  - /root/runtime_medium：HTTP/CLI/MCP/SDK/真实 MaiBot consumer；已完成本次入口后 interrupted。
  - /root/storage_medium：Source/Storage/Native Domain/installer；已完成本次 owner 后 interrupted。
- 更早 /root/requirements、runtime、storage 推理不是指定 medium，已经交接并停止，不能恢复旧设置冒充当前三个。
- 历史代理交接（temp）：temp/handoffs/ses-{runtime-bundle,storage,requirements}-20261028.md。
- 最新真实连接元数据（含凭据文件引用，不含把 key 放入代码的授权）：temp/review/product/connection.json；admin 路径从实例 run/host.json.bootstrap_credential.file 取。
- SDK 运行器：temp/venvs/nous-sdk/Scripts/python.exe；开发命令前 . ./scripts/dev-env.ps1。
- 服务任务 org.heptalogos.nous-00ed4814d13192f7。
- 实例 DB temp/instances/codex-product-20261026/data/authority.sqlite。
- 临时实际结果：
  - import-unit-frozen.json / import-unit-catalog.json / import-unit-processing-submitted.json
  - import-unit-output.json / import-unit-memory-source.json / import-unit-live-summary.json
  - import-unit-hybrid-query.json / import-factual-visible-contract.json
  - import-unit-native-preview.json / import-unit-native-export.json / import-unit-native-installed.json
  - import-unit-native-mapping.json / import-unit-native-roundtrip-summary.json
  - import-unit-receiver-query-before-restart.json
  - import-owned-cancellation.json / import-time-llm-actual.json
  - actual-chain-restart-summary.json（更早已完成链，不能冒充本批重启）
- 临时部署失败：temp/runtime/source-lifecycle-stage.log；temp/review/product/source-lifecycle-staged.json 当前是错误输出。
- 一次性离线调整草稿：temp/runtime/refresh-file-admission-schema.py，未执行。
- 本批未建 PR。临时结果、数据库、凭据与下载不在 Git，交接到另一台机器时不能假定存在。

## 10. Verbatim Anchors

- 最新用户指令：“立刻提交当前工作，并执行 [$session-handoff] … 文件放进根目录。然后，停止。”
- 用户架构要求：“模块化、低耦合高内聚和事务中心化，内部状态统一、规范。出问题直接改对应负责部分，而不是在用的地方亡羊补牢。”
- 用户对模型与 Prompt：“系统会给模型看到什么，期望得到什么，都得让我能看到。”
- 当前未完成不是授权缺失：用户已经授权开发和真实依赖使用；暂停来自最新明确停止指令。
- 未来恢复本文件后：等待用户下一条指令，不自动恢复 Goal、子代理、编译、部署、数据调整或任何继续点。
