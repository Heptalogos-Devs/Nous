# 全仓重整实际观察（2026-10-09）

[返回研究入口](README.md)

本报告保存本轮实际复现与结果，后续追加同一任务的新观察。任务范围包括 owner 结构、配置身份、执行、consumer、Serving、Portable 与成果保全；当前观察不证明整项重整已经完成。

## 2026-10-10 全仓语义合同审查续作

本阶段按完整十四项审查继续，之前的 Portable/consumer 资格是历史阶段结果，整项目标仍未完成。当前主实例和已保存的认知、回执、研究、原始矩阵与 SAUC 材料继续保留；未启动新的付费模型资格运行。

- Projection 直接使用生成合同，CoreCognition 接管宿主中的 query/projection/context 编排。既有实际编排与 Protobuf 场景保留 supports/contradicts，关系变化产生 RESET。
- Schema 的 evidence 只有 content 内一个位置。create/revise/split/merge 共用内容、formation 校验与 revision writer；split/merge 自己保持锁、epoch、lineage 和原子性。公共场景实际创建嵌套 evidence，真实数据库场景验证 producer、lineage 与失败回滚。旧 create/revise exact receipt replay 与后续修订均通过；原有 split/merge committed replay 缺口留在持久操作收口项中继续修复。
- Core Point/Range 贯通 canonical Proto、NousQL、TS/Rust 和 SQL。Point 匹配精确 instant 或包含它的 interval，Range 左闭右开且非空；unknown 不匹配。冻结时钟、relative/as-of 和 absolute 时间保留完整 Timestamp 精度。SQL 在 PostgreSQL 微秒存储格上按比较方向编码边界，保留原 predicate，不将点扩成 epsilon。
- 各语义 owner 提供 QueryFacts，共用 authority、entity、source include/exclude、role/mode、modality 和 epistemic 规则。公共 Schema exact 场景真实拒绝不匹配条件；各 owner 保留生命周期、权限和时间解释。
- 删除闲置 ExactPostings 的 need/build/load/invalidation/snapshot/trace 路径。物理资产由一个 AssetFamily catalog 描述；旧资产退出 current 后仍受 reader、research pin 和 grace 保护。实际回收/pin/reopen 场景通过，语义 Exact 与 owner SQL 保留。

本阶段没有增加 TS 用例，仍为36份/107项；既有场景增加针对上述真实裂缝的断言。仅新增一条此前无覆盖的 Schema split/merge 原子轨迹。Rust 私有 tests 保持父模块可见性迁往 tests；Core query 按 vocabulary/request/result 分类，Schema content mutation 与 lineage 分开。VCP23个原数值场景按机制归类，原 corpus 不改；Kernel query 和 concept 轨迹分职责、共用原临时数据库。当前用户长度配置为 TS600/800、Rust500/700，适用于测试和 helper，没有目录豁免；三个拒绝文件完成职责拆分后为0reject。Owner unit/golden、公共 Schema/时间、Material/Query、Serving/lease、replay、组织后的 Kernel 场景、TS、Clippy 与 fast checks 均有实际通过结果。

余项继续围绕 owner selection/provenance、bounded embedding/history、typed workflow/errors、OS lifetime mutex/supervisor、ReleaseInputs 和 command declarations 展开；Runtime 清理职责、embedding-space、deriveMaterial、其余 Rust tests 和现行文档/section anchors 尚待收口。PR24保持 OPEN/Draft，不合并。

## 2026-10-10 Linux 持久操作续作

从 `9fb8046` 继续。共享 lease、snapshot、maintenance claim、依赖引用、purged 终态与 execution telemetry 进入 canonical Proto；Core 的 formation、derivation、Resource admission 与 maintenance 共用持久操作协作模块。领域 payload 保持 owner 校验。Persistence 使用正式 envelope 与依赖集合，旧 JSON 字段推断、telemetry SQL 拼接和递归 purge 扫描退出当前路径。

受控数据库场景复现并保护 commit-to-host-ack 窗口：Host 先登记独立 child mutation ID，Memory 提交时同事务发布真实结果引用；在 Host 保存 outcome 前 purge，workflow 正文立即清除，旧 lease 无法恢复 proposal。旧 snapshot 的真实 maintenance claim、认知形成时间、proposal、unknown usage 与 opaque UUID 经一次性迁移保留；opaque 内容不被当作引用。Schema split/merge 成功重放返回原 exact results，来源已 withdrawn 不阻止已提交回放；旧 split parent-only 回执的单批次 lineage 恢复在临时数据库中实际验证。

当前 Linux 的 TS、focused owner/database/history 场景、Kernel 全 targets Clippy 与 fast checks 通过；Model/Material/Resource 和 longitudinal 公共 Core/Kernel smoke 通过，模型为本地受控替身。未运行新的付费模型资格，也未改写 operator 认知数据或配置。历史投影的 catalog、Tag descriptors、cognition metadata 与 Episode fragments 保持同一 view/transaction，按实际职责整理；原合同场景保留。

完整十四项工作仍未完成。typed domain errors、OS lifetime mutex/Core supervision、ReleaseInputs、command declarations、Runtime 清理所有权、embedding-space、deriveMaterial 与文档/anchor 治理仍需继续。PR24保持 OPEN/Draft。

## CLI 正文与明确拒绝

基线 `09d67d4452eead78346877980f804ac513675d8d`，Windows 上使用当前源码 CLI environment 连接已有 Dogfooding 实例的 authenticated public Core。每个探针使用独立 consumer state root，既有 Subject 与 WorkContext 不改动。调用无效 `CreateSubject(subjectId="invalid-uuid")`；该请求没有创建有效 Subject 或调用模型。

基线连续 257 次尝试：前 256 次 Core 返回 `InvalidArgument`（code 3），却保存 256 个 `pending`；第 257 次本地返回 `RESOURCE_EXHAUSTED`，没有到达 Core。呈现 `{text:"33333333-3333-4333-8333-333333333333"}` 丢失正文。

修复后独立 consumer 的同一真实路径连续 257 次均返回 Core code 3；256 个保留回执均为可回收的 `rejected`，新操作可以继续。完整 UUID 正文保真。焦点检查同时区分 UUID/ref title 与正文、exact evidence locator、未知结果原请求恢复、恢复后的明确拒绝，以及成功结果在呈现失败时仍可读回。成功回执读回不重发业务调用。

后续共享状态检查又复现了一条结果分类缺陷：Authority 明确拒绝后，拒绝消息使终态回执超过配置文件预算，本地 `RESOURCE_EXHAUSTED` 会覆盖原 Authority 拒绝。当前保留原错误，并带原 receipt 与 `RECEIPT_UNSAVED` notice；焦点检查用实际文件预算与可控 Authority 错误保护这一已知结果。

本次验证运行的是修改后的 consumer 与已有 Core；没有将其记作全新数据库、当前 Kernel 或 Portable 验证。

## Query 与纵向检查精简

两份超限 Rust 集成文件按实际风险精简。删除 Query 中重复的 VCP 中间数学/协议字段回放和已有独立数值 oracle 覆盖的脚手架；保留 exact/frozen snapshot、rerank 后 Authority 变化、Material 时间、Schema lifecycle/provenance、Resource forged continuation、预算诊断与扫描边界。纵向检查删除重复 Protobuf/DTO、分页和通道回放；Experience/lease、Episode partition/late arrival、Journal provenance/purge、partial actions、维护恢复与历史仍在各自 owner 场景中验证，共用测试内的实例和来源构造。

保留场景均通过当前 Kernel/临时 PostgreSQL。新增 Serving 场景真实保留在途 reader、建立下一代、验证回收保护，释放 reader 后回收旧资产，再删除缓存并重启，在同一 Authority watermark 召回原对象。该场景按 generation family 定位资产，使用持久 metadata 与实际文件可读性证明重建。

Material 时间场景曾一次未召回预期 region；诊断重跑和移除诊断后的整组重跑均通过，未据此修改领域语义或删除该断言。该一次失败的原因尚未证实，后续综合实际使用继续核对这条时间/来源路径。

当前 `check:length` 覆盖 406 份手写源码，16 warning、0 reject、0 read error。当前全 workspace Clippy、`check:fast`、TypeScript tests 和文档导航通过；这些证据不替代未完成的全仓重整与真实模型/Portable 任务。

## 手写源码长度门禁

门禁从单一 TOML 读取门限，已纳入 `check:fast`。临时 Git 目录验证达到门限的 warning/error、CRLF/LF/孤立 CR/BOM、未跟踪新文件、工作树修改、已删除文件、生成排除及未知配置键的 exit 2。起始基线有两份超限 Rust integration test 与旧研究 runner，已按上述实际风险精简；长度拒绝没有基线豁免。

## 配置规范化与身份

改动前的 owning Rust 检查实际复现了两种身份差异：只更改 descriptor exposure 改变 registry identity；同一 key 集合改变排列或增加重复项会改变 subset identity。Core 的真实 `video` 配置在省略默认字段时，启动交接中的 JSON 与显式 defaults 不一致。

修复后 Rust owner 检查确认 exposure-only 的 Catalog identity 会变，effective identity 不变；subset 按集合计算。Rust typed policy 与 Core Zod policy 在进入 snapshot 前规范化，不采用通用 JSON Schema defaults 解释器。

使用新构建的当前 Kernel 和源码 Core 运行隔离 configuration smoke：同一 operation ID 的 `video={max_frames:4}` 与完整显式 defaults 回放得到同一 revision/desired digest；desired 读回完整 owner 值，active 在 RestartProcess 前仍为原值。停止并重新启动后 active 读到该完整配置。公开 CLI config check/list、scope precedence、回放和 restart 同时通过。该临时实例没有模型调用，退出时停止并清除；既有 Dogfooding 实例保持运行。

## Terminal/MCP 共同执行

移除 MCP 每次命令启动 CLI 子进程的路径。当前 Terminal 和 MCP 在同一命令核心中解释 argv、执行 handler 和格式化结果；Terminal 的 stdout/stderr 与可消费 stdin 由 Terminal 入口持有，MCP stdin 只供协议。

真实 SDK stdio client 启动当前 MCP server，执行 NousQL help、JSON command help 和未知命令。握手及逐次消息均由 SDK 成功解析，help 结构可读，未知命令为 MCP `isError`，没有额外 stderr；不需要启动 Core 或调用模型。这验证协议纯净性，不替代后续真实 Codex 认知任务或长机会执行验证。

共享 consumer 改动另用当前 Core/Kernel 和干净 PostgreSQL 实例实际验证：配置 `consumer_state.receipt_limit=2`，Terminal 创建并保存 Subject，随后真实 CLI 子进程与 SDK stdio MCP 进程共用 state root/consumer 并发打开 Session、创建 WorkContext。两项选择均保留，MCP 能读 Terminal 保存的 Session，Terminal 能读 MCP 保存的 WorkContext；Core 重启后实例身份与 consumer 选择保持一致，MCP stderr 为空。临时实例退出时停止，数据保留在本轮临时目录等待统一清理；该场景没有模型调用。

焦点检查复现并修正旧 BigInt tag 与用户 JSON `$bigint` 键碰撞。四个真实 Node 进程并发写容量为 2 的 pending 空间，恰好两项成功、两项收到容量拒绝；未知请求不回收。同 operation 的不同 frozen input 被拒绝，终态不被 pending/另一个终态覆盖。连续 query 的清除旧结果与保存新结果使用本命令最近成功写入的基线，第二条 query 的 exact `result:N`/UseEvent 对应第二次 Client 返回；该焦点检查使用可控 Client。


## Resource 宿主与接纳

生产 Core 移除 RAGFlow 专用 wire adapter、配置 owner 和专属测试。宿主在启动时按 `adapter_kind`/`provider_profile` 显式提供唯一 adapter；LocalDocuments 不再继承 registry 或覆盖其解析方法。公开 descriptor、query continuation 与 Material 接纳继续使用原语义 owner。

当前源码 Core 的隔离 Model/Material/Resource smoke 已改为 LocalDocuments，实际通过公共 API 验证接纳、重复 operation 返回同 Observation、新接触保留独立 Observation、版本变化拒绝、权限撤销拒绝、已接纳来源在外部撤销后仍可读，以及 1 MiB 普通/控制字符材料与恢复。模型部分使用本地协议替身，不计作火山方舟真实模型质量或持续任务结果。

恢复检查另用可控 Kernel 替身确认了旧路径的缺陷：已有 proposal 在来源变更或权限撤销后仍会提交。Core 现在在接纳前核验实际 provider identity、当前版本和权限，包括 saved proposal 的恢复；已完成 outcome 则继续复用已接纳快照。该检查保护恢复窗口，真实 Core smoke 保护公共 Resource/Material 行为。

## 本轮真实 Codex 与模型续接

使用本轮独立 PostgreSQL/Artifact/cache/config 主实例、当前 Kernel、源码 Core 和官方 consumer。真实 Codex CLI 通过 Nous stdio MCP 保存两项实际重整来源、Subject 与持续 WorkContext；后续 fresh Codex 进程恢复同一 Subject/WorkContext，打开独立 Session 并继续。

首次非交互 CLI 因本机工具审批默认值与 `never` 冲突未执行认知操作；只为本轮三个已授权 Nous 工具配置官方 per-tool 模式后可调用。首轮观察的 `rebase://` locator 不符合 web source HTTP(S) 合同，改以原文观察保留内部 locator 映射。两次 formation 与 embedding 最初返回降级，query 明确拒绝：新实例复制的历史模型配置仍指向无 listener 的 `127.0.0.1:18002` 研究代理。当前 New API `/api/status` 与认证 `/v1/models` 均为 HTTP 200、所需模型存在；仅本轮实例 gateway 改为当前 `127.0.0.1:3000/v1` 并正常重启。旧实例、operator 原配置与 credentials 未修改，原失败操作身份和失败结果保留。

fresh Codex 进程读回原来源后，使用新 UUID 执行已明确失败且无 Memory 的两项 formation；两项均成功，trace 标明实际 `doubao-seed-2.1-pro`、`pro_minimal` 与 whole-Observation grounding。模型生成正文、exact revision、trace、原始来源和实际检索结果保全在[本轮 consumer 续接结果](corpus/deep-rebase/consumer-continuation-2026-10-09.json)，没有把旧内部回执读入新 consumer 格式。

| 原始 Observation | 形成的 exact Memory revision |
| --- | --- |
| `obs:lihih-titij-popad` | `memrev:sajot-zujup-muhug` |
| `obs:dutob-dasad-pokol` | `memrev:raman-zajos-ruvuj` |

原 Observation 在形成前后均完整读回、原文不变。Embedding preparation 实际提交 6 项、请求 2 次。普通自然语言 query 在 `baseline-rrf`、concept enrichment `off` 下完成，consumer revision 为 rank 1，报告 lexical/dense/language-rerank 参与；一个 referenced UseEvent 接受计数 1、重复计数 0。WorkContext `ctx:raraf-romup-hasal` revision 6 保存 Objective/Decisions/Current Work、exact refs 与失败 ledger，读回确认 open；fresh Session 关闭。当前 Subject `sub:tujap-vibuz-gozod` 保留供后续持续任务与 Portable 续接。

旧实例 consumer 记录中找到的两个 pending（UseEvent 与 formation）已按原始字节复制并逐份 SHA-256 核对，manifest 保留原路径、receipt identity 与 Subject；原目录仍保留。其具体内容位于 ignored 本地研究成果区，不加入公共结果文件。Linux SAUC 独立硬盘提取、全仓剩余 owner 重整、媒体 route/deadline/lease 与 Windows source-less Portable 尚未完成。

## 模型输入、fallback 与执行 owner

两组不同音频能力的实际配置经受控 HTTP 复现了原条件：首选有 audio_input、fallback 没有时，首选返回 503，fallback 的音频观察错误通过并被提交；反向配置则误拒合法音频输出。另一 owning contract 检查复现原始帧与 Transcript 共用 original 标签时，转写内容被允许声明为直接听到。

当前 Model input owner 每次 attempt 按实际媒体和冻结候选 profile 过滤输入、构造实际访问声明；Material interpretation owner 使用这份声明校验。visual/audio/source_text 各自保存 original/representation/unavailable，Transcript 不取得直接听到的身份；description 第二阶段读取实际持久 quality 而非首选模型能力。当前公共 Core/Kernel/PostgreSQL 的 smoke:media 验证两组配置、持久 quality.input_access/payload.evidence_access 一致，以及成功重放不增加 provider 请求。该 fixture 使用 opaque bytes 与受控输出，不声称火山媒体质量。首轮临时 PG setup 曾超时退出；同一检查重跑通过，未修改启动超时，失败未作为成功证据。

实际 HTTP cancellation 检查另复现旧 telemetry 在 fallback 中断时整体丢失；当前保留首个 503 failed 与已传输的第二个 unknown/caller_cancelled attempt，未知 usage 保持未知。完整 SDK/raw-media 响应已知 usage 在输出校验失败时保留。独立 profiles、协议 adapter 与 Material interpretation 移出 invocations；没有依赖其私有状态的 part 类或转发兼容层。协议读取使用 get-stream 的字节上限替换两份 reader 循环。

手工 material projection/validation version 常量已替换为同一个 owning code identity。Source/bundle 探针确认源码与 source-less ESM 程序的 provider/material 实现摘要相同，bundle 内实际含 get-stream、proper-lockfile、write-file-atomic 与 devalue，CLI JSON help 可解析。该证据只覆盖当前 bundle 与执行身份，完整 Windows Portable 连续任务、deadline/lease 统一与其余全仓 scope 仍待完成。

## 执行机会与确认期限

受控 Client transport 时钟复现：120 秒授权工作后仍需要 8 秒确认，旧 `maxElapsedMs + 5000` 在第 125 秒返回 deadline。当前 Core 与 Client 共用 `core_execution.opportunity` typed policy，工作、cleanup、need acknowledgement 和 response margin 分开命名。Client 从 active Configuration 计算实际等待；显式更短 caller deadline 保留。每条 provider route 的 timeout 继续限制单次调用，不能重置工作机会。

Core 的 ordinary formation/derivation/embedding/resource 工作使用同一 work clock，private ModelWorkflow reservation 显式携带该机会的剩余 lease，删除另一个 `model_workflow.lease_seconds` 默认值。maintenance 父 claim 至少覆盖 worker policy 与相同 work/cleanup/ack，child 仍验证实际父 token 并覆盖父期限。取消后 telemetry/save/release 共用有界 cleanup，finish 使用独立 acknowledgement。现行 PG workflow/maintenance 四项场景通过，包括 proposal/terminal replay、父期限覆盖与 runtime rows 回收；30ms 机会到期检查确认 unknown attempt 以有效 cleanup signal 保存，need 返回 pending/deferred。

公共 Core/Kernel/PG smoke 额外以 1 秒 work budget 中断已传输且保持 pending 的 fallback。响应是 Code 4 DeadlineExceeded，cleanup 后相同输入立刻恢复成功，没有遗留 busy lease。该过程使用受控 provider，仍不声称真实媒体质量。持续实例正常重启后 active policy 读回为 work 300000ms、cleanup 10000ms、ack 5000ms、response margin 1000ms，Subject/Runtime/Memory 与当前 model roles READY。TypeScript 33 files/109 checks、check:fast、Knip、依赖边界、docs navigation、Rust workspace check/Clippy 与当前 source-less bundle 探针通过。Query retained execution lease ownership及其公共等待、完整配置 graph/CAS/leaf、Windows Portable 连续任务和成果保全后的清理仍属于原目标未完成部分。

Fresh Codex 第三次在同一 Subject/WorkContext 完成真实续接。531 字节新来源 `obs:bojin-gagof-pujaz` 原文形成前后保留，Doubao Pro/pro_minimal 形成 grounded revision `memrev:bufor-rurov-pupik`，trace 关联 wholeOccurrence。Embedding committed 9/requests 2，指定自然语言 query 召回该 revision 为 rank 1，referenced use accepted 1/duplicate 0。Context revision 7/open、Session closed 读回确认，19 份原始 public 结果保存在 [续接 corpus](corpus/deep-rebase/execution-continuation-2026-10-09.json)。该实际 trace 还暴露 Formation 的 ProducerSignature 仍覆写为手工 SDK 版本常量；原输出及已有 cognition 保留。公共 smoke 加入实际 producer readback 后先失败于这个旧常量，再在 Formation/Material/maintenance 共用 metadata owner 映射后通过。当前执行 snapshot 在发出请求和预算 admission 前核验固定角色的 output schema digest，避免用新合同解释原固定执行。

## Query 同一机会的生命周期

真实 PostgreSQL 检查以 2 秒执行 lease 复现：原 preparation 在 1.4 秒后取出、重新保留，超过原始期限仍能读取。旧 PendingQuery 在每次 retain 使用新 created/default duration，等同于激活延长机会。当前 Runtime QueryReservation 分开持有 BoundQuery 与一个 QueryLease，新的 preparation、execution ticket 和 final validation 继承相同 infrastructure deadline；删除独立 `runtime.query_lease_seconds` 默认值。Core 的编译、Query/model/resource/projection 阶段共用执行机会，Client 也从 active policy 派生等待。原机会截止、验证票据到期、配置/context 冻结、Authority lifecycle/epoch revalidation、Serving reader 回收与公共纵向检查通过。

当前 Core/Kernel 正常重启后，真实 CLI 在同一 Subject/WorkContext 开 fresh Session 执行自然语言 Query。首个结果因 producer identity 重整后的 compatible embedding material 缺失而 degraded；显式 prepare committed 9/requests 2，随后相同 query complete、召回原 `memrev:bufor-rurov-pupik` 为 rank 1，dense/language_rerank 证据保留，Session 已关闭。首次失败和恢复的六份原始 public 输出保存在 [Query 续接 corpus](corpus/deep-rebase/query-continuation-2026-10-09.json)。这次续接不重写原 cognition/history，不替代剩余配置图、全仓 owner 收尾和 Windows Portable 验证。

## 模型配置图与冻结

三项 owning 检查先失败：generation/gateway 默认 tokens/timeout 尚未进入 normalized graph；显式缺失 route 仍被接受；无关 dormant model 和 video budget 改变 formation snapshot identity。当前 `models` 将紧密耦合的 gateway/model/execution/role 作为一个原子 policy 发布；真实输入与 defaults 在该 owner 完成规范化，再交给 Kernel Catalog 冻结。非法引用/协议/推理组合给出精确路径；缺凭据、disabled gateway 和 unset model identifier 保留 capability state。Role snapshot 与 embedding producer 使用同一依赖子图，只捕获相关角色、execution、model 与 gateway；删除调用处补4096的 resolver。

当前 public configuration smoke 验证 sparse graph 与完整默认 graph 同 operation replay、desired normalized tokens/timeout、错误 route 的精确 public 路径及 restart 生效；Model/Material/Resource、相反媒体能力和期限后恢复 smoke 通过。34个TS文件112checks、typecheck/lint/Knip/dependencies通过。独立字段 leaf descriptors、修改 revision CAS、更多实际 effect/apply mode 和完整 Portable 仍须继续，不能从该模型图检查推导全配置已完成。

## 配置修订与回执重放

公开配置 smoke 先复现旧 Kernel 接受过期 expected revision、覆盖整块模型图的缺陷。当前 canonical Proto、Official Client、CLI 和研究调用方共同携带 expected revision；公开 mutation 缺字段返回输入错误，CLI 写入要求先读取 desired revision 并显式提交，避免重试时重新读取修订而改变请求身份。Kernel 在配置事务内比较并递增全局 revision；冲突不写 override、不推进 revision，已完成原请求先读取冻结回执。

实际 PostgreSQL 集成检查验证同一 revision 的两项并发写入只有一项成功、过期 clear 不删除获胜值，后续修改后仍重放原 operation 的 outcome；该 owner 检查同时保留独立字段无条件内部写入的串行完整快照合同。公共 Core/Kernel/PG 与 CLI smoke 验证过期模型图拒绝且 desired 不变、CLI 缺 revision 拒绝、过期 clear 拒绝、Subject 修改后原 CLI operation 可重放，并完成 restart readback。configuration 五项集成检查、TS34文件112checks、check:fast 和 Rust workspace all-targets/all-features check 通过。独立 leaf、effect/apply 与完整 Portable 等原目标仍未完成。

## 独立配置字段与实际消费者

Catalog 检查先确认 `video.max_frames` 等真实叶路径不存在。当前 audio/video、Material inputs、Core execution/opportunity 和 official consumer state 的独立字段直接使用原 Zod owner 发布 descriptor，不再注册重叠 parent；模型引用图保持一个完整原子 policy。Core 从 Kernel 已解析叶值重组 typed group，Client deadline 与 CLI consumer state 读取同一 active 叶值；离线检查和研究合同 inspection 共用 registered owner 路径提取部署值，删除重复 material 字段拼装。单位随字段发布，媒体/Material 标注 authority formation、consumer composition/rerank candidate budget 标注 query policy，实际仍需要重启的参数继续返回 RestartProcess。

FFmpeg executable reference 使用 null 作为已安装 runtime pack 的明确选择；string 是显式引用，空 string 非法。公共配置检查实际验证 max_frames 的独立范围、另一个 input_mode 保留默认、旧 parent 不可 describe、单字段 override 重启生效，以及清除该 override 后 desired 回落到部署值而 active 保留原值。公共模型/材料/资源、两种媒体能力、work deadline cleanup/resume 与纵向 Query/维护检查通过，TS34文件112checks、check:fast、Knip 与依赖检查通过。

只对本轮持续 Core 正常停机并换用 SHA-256 核对的当前 Kernel 副本后重启；实例 ID 保持 `ffe03b02-8997-476e-b8d1-370e88aac1a2`，新 endpoint 为 `http://127.0.0.1:38930`。active opportunity 四个叶值读回 300000/10000/5000/1000ms，Subject/Runtime/Memory 和11个 model roles READY；atomic models 仍为6个 model/11个 execution profile。真实 CLI 指定原 Subject/WorkContext 后读回 revision7/open。首次未指定 Subject 的默认 consumer 查询被明确拒绝，没有修改选择或认知状态。旧实例和 Linux 硬盘未清理；完整 Portable、只读批量地址呈现、其余 owner/fresh SQL 重整与原目标仍待完成。

## 地址目录的只读批量入口

当前原 CLI friendly renderer 和 Core Query 呈现仍会调用逐项 Bind；检查同时确认多数地址在首次显示时才分配。新的 canonical `GetIdentityAddresses` / Official Client `identity.addresses` 提供单次目录 read，按 Subject visibility 隔离，保留目标顺序；缺失地址保持缺失，不分配、不改名、不增添 visibility。Tag 当前 lineage 从目录的 canonical Tag 规则解析，未知和 tombstoned 引用返回不同状态。

实际 PostgreSQL 的地址分配/冲突检查扩充验证批量读前后 lexical bindings、visibility、Authority history 行数相同；不存在和跨 Subject 的引用不给地址，tombstone 不给可操作地址，合并 Tag 与 survivor 得到同一规范地址。现行 concept lineage/Query/Serving 场景和公开 Core/Kernel/Client smoke 通过，公开 read 后原 display name/aliases 不变。该入口只是 owner 能力；schema 明确的字段呈现、创建/显式地址服务的分配和原逐项 Bind 删除仍须完成，不把新增 read RPC 当成呈现已经迁移。

本轮持续 Core 正常换用 SHA 核对的当前 Kernel 后重启，instanceId 不变，endpoint 为 `http://127.0.0.1:37081`。早期 discovery 尚未生成，原启动进程确认仍活跃后再次观察 READY，没有因此重新启动。新 public batch RPC 读回原 `sub:tujap-vibuz-gozod` 与 `ctx:raraf-romup-hasal` 的相同 lexical addresses；原 WorkContext revision7/open。Subject/Runtime/Memory 与11model roles READY，opportunity 四叶保持原值。该轮没有新增付费模型调用或改写认知历史。

## 协议引用呈现与创建事务

Client JSON 保真检查先复现 `$typeName`/`$unknown` 及嵌套内容被当作协议字段删除。当前 data codec 按真实 protobuf descriptor 处理 Value/Struct、oneof、list/map，保留 JSON opaque leaves；schema 使用 WeakMap，consumer 的 devalue 类型记录保存并恢复实际 schema，包括 timestamp dependencies。任意 JSON 键不获得协议身份。配置值与 schema/default 的三套单独转换收敛到同一 owner，CLI 继续只依赖 Official Client。

Canonical Proto 标注实际引用字段；CLI renderer 删除 UUID/字段名推断和逐项 Bind，按声明的引用去重只读查询。Source locator、正文、title 和 JSON 数据保持原值。Query diagnostics 使用 owning command 的明确投影，developer trace 保留。创建 owner 在同一 Authority 事务内发布对象、immutable revision 与 Material 地址，保留已存在的名称/aliases；Core Query 同样只读取得 exact preferred revision 地址，地址服务故障不抹掉已取得的 Query 结果。Subject seed adoption 与 Journal revision write 使用各自领域 payload，不以 receipt kind 推断分配，也不添加通用 CRUD 状态机。

公开 CLI 首次创建检查先在旧 Kernel 下返回 UUID而失败；当前 Kernel 下返回稳定 `sub:` 地址并通过完整 configuration smoke，包含真实 provider-options JSON 同名键的 normalized graph/readback/replay。独立 receipt 检查验证 schema、BigInt、timestamp 和同名 JSON 键往返。35个TS文件112checks、check:fast、Knip/dependencies、workspace Clippy通过；16项 Native Authority/Material/Memory/Schema/Journal/Episode/Tag/preparation 场景与公开 Model/Material/Resource、相反媒体能力、期限后恢复及纵向 smoke通过。这轮受控 provider 不计作真实模型质量；当前持续实例升级与完整 Portable 等原目标仍待继续。

持续实例已换用 SHA 核对的当前 Kernel 并正常重启，instanceId 保持不变，endpoint 为 `http://127.0.0.1:13751`。当前 public batch 读回原 Subject/WorkContext 的同一 lexical refs，Context revision7/open；普通文本 CLI 的 `context show` 实际返回 `ctx:raraf-romup-hasal`，结果原样保存供续接。Subject/Runtime/Memory 与11model roles READY，opportunity 四叶保持原值；没有新增付费 formation 或改写旧 cognition。

## 初始数据库与 Portable 恢复

现行数据库 schema 收敛到 `0001_foundation.sql`–`0004_indexes.sql`，直接声明 WorkContext text/anchors、Formation basis、ProducerSignature 字段与 workflow execution telemetry。删除三份旧增量 schema 及其递归 JSON 改名逻辑；Core/Kernel 同时删除没有独立语义的私有 bootstrap `bundle_revision`。认知 revision、epoch、来源身份与配置 revision 保留。首次检查发现 Schema evidence 的 `support` 判断被机械改名，恢复原语义后 configuration5、historical2、memory5、model_workflow1 场景通过。

原持续实例先保存完整 PG archive，正常停机固定数据，再将 data-only 恢复到由四份当前 schema 初始化的新库。新库保留自己的 `_sqlx_migrations`，没有重写旧库迁移账本或加入旧 payload decoder。70张业务表、196行逐表 SHA-256、列合同与序列位置全部相同；原数据库、Blob、operator configuration、Secret 和两份完整备份仍保留。停机备份摘要为 `bbc34f0cbdd87e6db85b1dc630a7081ed76b14e84773e97305ccba1b8657c0f5`；本地 archive、数据摘要和恢复 manifest 位于 `data/research/results/deep-rebase-preservation/fresh-schema/`。新库公开读回同一 instanceId、Subject/Context revision7/open 与三个原 Memory trace。

Windows GNULLVM release Kernel、两个 private DLL 与 source-less Portable 已重新构建。独立 home、colocated、独立 locator 后移动安装目录三种布局均通过 ZIP inventory SHA、SBOM/许可证、实际 boot/restart、固定数据库端口和缺失 PostgreSQL pack 拒绝检查。受控 wiring 结果不计作真实模型质量。该包的2333个文件摘要再次核对后，使用独立 locator 连接恢复库、原 instance identity 和 operator roots；只用 bundled Node/CLI/Client，Core 的 PATH 仅含 Windows System32。公开读取继续返回原三个精确 Memory trace、Context revision7/open、Subject/Runtime/Memory 与11个 model roles READY。

此前8f28074的 Windows CI通过，Linux CI在 Resource continuation 发现 nullable `display_label` 被地址分配路径按必填 String读取。Resource owner改为先解码 typed descriptor，使用可空 label 发布地址；原失败场景及 Query correctness五项通过，workspace all-targets/all-features测试、Clippy、check:fast和格式／文档导航检查通过。

Fresh Codex 经编译后的 Portable MCP，在同一 Subject/Context 完成第四次真实模型续接：545字节 `obs:lalof-sarus-muzis` 形成 `memrev:jazop-nakis-tipum`，实际 Doubao Pro/pro_minimal、wholeOccurrence trace；embedding committed12/requests2，指定自然语言 Query complete、该修订rank1，referenced use accepted1/duplicate0。Session关闭，Context revision9/open。随后正常停机、用同一 locator 和 bundled runtimes 重启，公开读回 instanceId 不变、Context9/open；三个原 Memory trace 的 JSON字节与重启前完全一致，摘要留在本地实例。

重启后的另一 fresh Codex恢复原Context与已关闭Session，未新增 formation、embedding preparation 或 use；同一自然查询 `01a12061-8db3-7ec0-968f-64a92d599a3d` complete，四个 Memory精确修订依次召回，新修订rank1，lexical/dense ready。新Session正常关闭，Context保存并独立读回 revision10/open。两次实际任务的原始命令结果保存在[Portable续接 corpus](corpus/deep-rebase/portable-continuation-2026-10-09.json)。初始 projection wildcard unavailable和上下文 truncation也保留，没有将其替换成全部READY的叙述。

Windows只读盘点确认独立物理盘1的分区2为Linux filesystem GPT类型、无Windows盘符。只读 `ro,noload` WSL挂载及执行权限重试均返回 `WSL_E_ELEVATION_NEEDED_TO_MOUNT_DISK`；没有挂载或修改该盘。SAUC提取仍需管理员挂载或从Linux侧导出，相关旧材料继续保留。全仓其余原范围与最终CI／清理尚未完成。

## 旧 synthetic 功能执行器退役

按任务包第03项核对 Vault Target/Decisions 与当前 longitudinal、Authority/provenance、Runtime/Use、Query/Serving、WorkContext Specs 后，退役 `cognitive-functional.ts`、`NOUS_FUNCTIONAL_SMOKE` 分支、专属文本匹配 provider、CLI 子进程脚手架、package/Knip/script入口及活动说明。手工三场景、22事件、15 intents、四 profile 和 selected raw-text source/oracle 保持原文件；五份语料及历史说明另按原字节/SHA保全到 `data/research/results/deep-rebase-preservation/functional/`，未清理旧 raw 数据。Spec 中的旧实验执行状态移出活动合同，其原事实继续留在研究语料说明。

纵向 public smoke仍实际执行有界维护、模型 proposal、owner提交、重放和Core/Kernel重启。Concept局部catalog/Accretion、Tag lineage/历史/目录与 typed Core partial-commit/transport-resume 的独立检查保留：受影响Native5项、Core9项通过；check:fast、Knip、依赖边界、文档导航和diff检查通过。脚本从902行降至452行，删除一份687行旧runner；长度门禁422手写文件、16warnings、0reject。这轮没有削减 production功能或改动领域语义，仍需按完整目标完成其他实际操作与成果保全／清理。

eadb42a的Windows CI通过；Linux未进入测试，显式runtime准备调用第三方GitHubrelease API返回403。已核对安装的 `postgresql_archive` 实现，其支持 `GITHUB_TOKEN` Authorization；CI仅给runtime准备步骤传递现有contents-read job token。没有跳过准备或降低验证；新head的实际CI结果另行确认。

9187d7c的Linux/Windows CI均通过，Linux明确完成runtime准备及完整`just check`。随后通过Windows正常管理员确认，以核验过身份的独立物理盘1/分区2执行`ro,noload`挂载、保全并卸载，解决此前非管理员进程无法读取的问题。实际Linux来源为`/home/arsvine/Services/new-api/`，补丁是独立的`volcengine-asr-transcription.patch`；36,724字节，原文件和Windows提取文件SHA-256均为`e2867c0b7a8558bb6e1ed542bcf0c6580b84a5217174e6133f3db1a8b6ada426`。源码、Git目录、构建/部署文件和私有operator配置同存于 ignored 保全区，原盘未改动且已卸载。

Linux源码checkout为clean，HEAD`271976055fcbd75a7180d6ca8389ec8c80fa4437`，已经含SAUC/WebSocket实现。Windows恢复的Git HEAD与原盘一致，`HEAD:relay/channel/volcengine/asr.go`与工作文件归档逐字节一致，SHA`b62239f8c9bf27eccf36d55a1b9361734f91cc4f65c16a6f14ddfbab3752946e`。原补丁包含多段增量及Responses修正，直接对当前已修改源码再次单次apply-check不通过，未据此改写补丁。该仓库是partial clone，全refs bundle因缺promisor对象失败；完整本地Git目录和当前源码已按字节归档，没有下载未在原盘保存的上游history。补丁和当前已提交实现的可读保全已经验证，Windows New API运行服务未修改或部署。

已有真实SAUC成功结论、Agent Plan channel、24kHz→16kHz规范化、Menon40秒WAV约7.2秒转写、MP3及production Transcript/formation/Query结果继续由[2026-10-04研究记录](observations.md#asr-与-responses)和原source manifest拥有，本轮不重新命名为当前运行结果。本地精确补丁、配置与archive/verification位于`data/research/results/deep-rebase-preservation/linux-sauc/`；密钥值没有进入Git或报告。

## Linux 接续与 Serving 身份

Linux 从最新 `master@09d67d4` 核对后接续同一分支的 `d5ca936`；Windows checkout 工作树干净，现有 Draft PR 仍为 #24。Windows 两个数据分区以只读方式挂载，保全区复制到 Linux 私有成果目录。独立 Linux SAUC 补丁与 Windows 提取副本的字节摘要相同，运行中的 New API 未修改。Windows 的现行 Portable、Subject、Context revision10/open 与实际模型输出继续保留；不把此前完整任务重跑为新的成功记录。

剩余 owner 审查发现 Serving 仍保存手工实现计数，并在结构化 record 与 metadata 中复制同一身份。真实 PostgreSQL 回归将 record 的实现身份置为过期、保留原 metadata，原路径仍复用旧资产；修复后在相同 Authority watermark 上重建。实现身份改为投影代码、输入投影和锁定依赖的构建内容摘要，兼容判断只读取 owning record。当前与历史资产安装合并，Native Wave 的策略注册与图构建分开。独立目录中的实际构建脚本探针确认：投影源码变化改变身份、只改 Wave exposure 保持身份、LF/CRLF 与源码根移动保持身份。

参考默认族直接使用当前 JSON 文件和 identity，删除 `-v1` 文件名及重复 revision 包装；当前所有读取方同步替换，没有旧格式解析。数值未改变，历史研究身份和 corpus 保持原样。Linux `check:fast`、35份 TypeScript 测试、Rust workspace/all-features 测试、Clippy 通过；长度门禁424份手写文件、15个警告、无拒绝。配置、历史 Authority、同水位重建和 reader 回收的真实数据库检查通过。干净 Linux 实例配置已通过离线 owner 校验，实际 Codex/模型任务、剩余配置与 owner 审查、成果提取和运行材料清理继续进行，整项重整尚未完成。

干净 Linux 实例随后实际启动，公共 CLI 读回 Subject/Runtime/Memory READY 与11个配置角色 READY。新的 fresh Codex 仅通过 Nous MCP 完成一项认知任务：1125字节来源 `obs:govig-fumoh-rokad` 原文读回不变，实际 `doubao-seed-2.1-lite` / `memory_formation_text` 形成 `memrev:gizoz-zonas-hivin`，grounded/accepted/valid，trace 与源区域保留。Embedding committed3/requests2，自然查询 `01a120ec-d973-7b72-82b1-7ed92ea84d51` complete、该修订rank1；referenced use accepted1/duplicate0。Subject `sub:bizor-fusin-vakav`、WorkContext `ctx:supud-pasun-kopud` revision3/open 保存待续工作，Session 已关闭。首次 Use 错误传入 connection-owned consumer，被 MCP 明确拒绝；移除该参数并沿用同一 event identity/timestamp 后接受。34份实际命令输入/结果及 consumer 结论保存在 [Linux续接 corpus](corpus/deep-rebase/linux-continuation-2026-10-09.json)。

Linux 续接审查将 concept activation 的最大保留数、最小 cosine 相似度与 model catalog 数交回 Runtime 的三项 Advanced/SubjectOverrideAllowed/Live/QueryPolicy 叶。Retrieval activation 与模型 readout 读取同一冻结 ConfigSnapshot，删除直接静态 JSON 解析和未读 strength mapping；真实数据库检查验证先绑定1/1、更新3/3后旧请求保持1/1，新请求读3/3，并按 similarity 过滤正交 Tag。provider 响应的结构边界仍由模型合同独立拥有。

实际重启发现两处生命周期缺陷：Core 在收到 SIGTERM 后仍持有启动方 stdin，资源已关闭但进程不退出；移除该进程拥有的信号/end listeners 并暂停 stdin 后，开放 stdin 下直接 SIGTERM 正常退出且端口释放。Kernel 在 embedding 绑定前后各刷新全部 Serving，导致启动阶段不可用告警及无需求建资产；删除两处启动刷新，按请求准备及显式管理刷新保留。缓存丢失的真实 PostgreSQL 回归先复现启动即建资产，修复后启动目录为空、首次查询在相同 Authority watermark 重建并召回原两条修订。请求、当前及历史快照现在共用 owning asset install。

重复代码扫描使用配置的 apps/packages/crates/scripts 范围，移除覆盖该范围的命令行点路径；研究 corpus 的真实重复输出不作为代码克隆。原生 workspace/all-features、Clippy 与完整35份/112项 TS 检查通过；并行负载下两项子进程检查超时，负载结束后原检查通过，未放宽 timeout。

## Linux 重启、实际维护与成果退役

同一实例在 `de9ca74` 正常重启后，新的 fresh Codex/MCP 读回原 WorkContext revision3/open、原 `memrev:gizoz-zonas-hivin` 与精确来源、producer 不变；只接纳新增1273字节审查文本并形成 `memrev:lobod-rimil-nojit`，仍由实际 `doubao-seed-2.1-lite` / `memory_formation_text` 生成，accepted/valid、grounded。Embedding3/2、Unicode Query complete（原修订rank1、新修订rank2）、exact只返回原修订；as-of `2026-10-09T13:49:30Z` 的 history只返回原修订，明确报告 `future_context_ref_excluded`。新 Use accepted1，原、新 Memory 与 Tag 保留在同一 Context，revision5/open。无选择的 session show 被明确拒绝，属于调用方错误。完整结果在[重启续接](corpus/deep-rebase/linux-restart-continuation-2026-10-09.json)。

第一项维护 grant 如实返回 `disabled_by_policy`。公开 CLI 以读到的 configurationRevision1，仅对该 Subject 设置 `maintenance.enabled=true`；系统开关仍关闭，不产生后台付费循环。随后 fresh Codex 在一个新 Session 只授予一次2operations/2model calls/120000ms机会。实际2次调用、120024ms、`opportunity_exhausted`：episode_segment deferred，concept_maintenance deferred/`opportunity_budget_exhausted`，没有宣称提交完成。exact原修订与新修订保持 revision1/epoch1、来源与producer；查询召回新/原修订rank1/2但提示缺少 compatible embedding material。显式 preparation committed2/requests2 后，公开 Query `01a12126-3228-7f03-9922-2f4acbed44f4` complete、degradation空，仍返回同两条 exact revisions。对象级 show/trace、need UUID的 generic show 被明确拒绝，exact修订可读；原错误均保留。Context revision6/open、Session关闭，[有界维护](corpus/deep-rebase/linux-maintenance-continuation-2026-10-09.json)及[材料恢复](corpus/deep-rebase/linux-maintenance-recovery-2026-10-09.json)保存实际结果。

在删除前，将 Linux 8091份、Windows3798份独有结果分别整理为 UTF-8 JSONL/gzip；保持原模型正文、非法/失败响应、query/result与数值、来源日期/ref、Prompt/合同、usage和条件。只去重逐字节相同文件、删除 credential/媒体编码与可再生 embedding 数组；日志使用明确标注的相关末尾片段，失败模型响应另保全完整正文。所有记录逐项解压、解析及摘要核对；两份成果在两端硬盘均复制并重新核验。原 operator TOML、退役consumer请求和资格结果片段分别保留，现行密钥仍由 SecretRoot 拥有。成果路径为两端 `data/research/results/linux-preservation-2026-10-09/`、`windows-preservation-2026-10-09/`，SAUC与原24份独有 VCP数值继续保留；没有模型重新摘要或修正历史事实。

清理45个明确退役根：旧实例/实验 PostgreSQL、迁移和qualification材料、原始run/trace、下载正文和媒体、可再生Serving/embedding缓存、测试cluster、旧Portable ZIP/重复payload、runtime staging和重复工具下载。Linux保留 `deep-rebase-linux` 当前实例；Windows保留 `deep-rebase-foundation` 连续状态及 `continuing-20261009` source-less Portable。两端 operator/Secret和现行运行依赖不删除。保留Portable的2333个文件重新核对SHA全部相同；其真实三布局/重启/模型任务属于先前Windows执行，本段不声称在Linux重新运行Windows二进制。独立New API源码、SAUC补丁和正在运行的服务未变。

Linux公开Core/Kernel/PostgreSQL的Memory历史/清除、Runtime跨Session、LocalDocuments版本/权限、相反媒体输入、deadline cleanup/resume、纵向和配置CAS/replay/restart/readonly batch场景通过。longitudinal首次被本机缺少libstdc++链接别名阻止；使用本轮已核验的本地linker目录后原检查通过，未修改生产代码或跳过。Linux/Windows `1e6e8dc` CI通过；最终文档/保全提交另按 exact head 校验。

保全复核发现通用缓存筛选误将历史VCP输入的 `vector` 数组省略。清理前 manifest 已记录原文件完整SHA-256；本地Git对象仍保存全部原bytes，按摘要找到并恢复43份历史完整矩阵。原JSON逐份核对，JSONL内相应输入/expected/来源/容差恢复完整，再重新解析验证8091份结果及两端副本；没有重新生成数值或更改历史结果。完整原bytes位于 `linux-preservation-2026-10-09/numerical-originals/`。

## 2026-10-10 Windows 交接点

按用户最新要求提交上传并交接，停止本轮Linux执行，不解除Draft、不Squash Merge。主实例正常SIGTERM停机，管理实例已停机，两者数据库/Blob/Context保留；维护预算退出后的只读DB检查确认五类pending needs均无live lease，Memory workflow无live lease。`1e6e8dc`两平台CI已通过，随后仅增加当前文档与三份真实续接/恢复corpus；该交接提交CI另查。Windows checkout在同一分支，继续前应读取最新commit message与工作区handoff。保留的Windows Portable是此前实际验证的payload，尚未包含本轮Linux的Serving内容摘要、三叶concept policy与lazy startup更新；最新head的Windows native rebuild、Portable资格/实际续接、剩余重复内部api_version决策、最后build清理/审查与Squash Merge仍未完成。本轮没有再次启动Windows二进制。

## 2026-10-10 Windows 续接与当前格式收尾

用户明确要求核对 Linux 进度与 Spec 后继续完成原目标。Windows clean checkout 与 PR24 均为 `f5c88c2`；其 Linux/Windows CI 已实际通过。按原任务包的唯一当前格式要求，复核 Memory Authority、Runtime/Use、Query/Serving 与 Runtime Bundle Spec，删除 Rust CognitiveQuery、Memory/Kernel readiness status 的重复固定 `api_version:1` 和 `API_VERSION`。该值由 Kernel transport 自行注入，不承担协商或历史身份。Canonical Proto 已声明当前 wire shape；认知 revision/epoch、Cognitive Seed 来源版本、模型/Prompt/实现摘要保持其语义。所有 Rust constructors 同时替换，没有增加旧格式 decoder；无调用方的 `MemoryService.ready_status` 转发入口与专属模块一并删除。

最终 owner 复核沿 Source/Material/Memory、Query/Context/Serving、Use/Maintenance 三个操作闭环进行。TypeScript Core 持有公共宿主、实际模型/媒体执行和 Resource adapter；Rust semantic owners 持有 Authority、mutation、历史和生命周期；Persistence 只供应事务/数据库/地址机制，Retrieval 供应可重建 Serving，Runtime 冻结执行/概念策略，不依赖具体 provider。配置在 owning schema 规范化后冻结，Catalog exposure 与执行身份分开；当前 Proto、四份 fresh SQL 和内部 typed shape 同时更新生产者/消费者。保留的检查针对实际 Authority/CAS、拒绝与未知结果、历史/purge、来源接纳、相反媒体 route、取消后 lease/恢复、共享 consumer 和 Serving 重建风险；没有为了 warning 数量继续拆模块或增加 runner。本次没有改变已接受的长期认知语义；Windows Vault checkout `ddafa31` 较旧，按本地已有 accepted `origin/main` 精确对象 `5b96c63` 读 Target/Decisions，未改动用户的 Vault 分支。

Shipping GNULLVM Kernel 构建成功；Windows `check:fast` 覆盖423份手写源码、15 warning、0 reject，35份 TypeScript/112项检查、workspace/all-features Rust tests、all-targets/all-features Clippy、Knip/dependency 边界、Buf 生成一致与文档导航通过。第一次完整 Rust 检查在历史查询的 Tantivy `.fast` 写文件上报 Windows `PermissionDenied`（code5），该场景单独运行和完整重跑均通过，没有 Serving 代码改动；本记录保留首次失败，不宣称其原因或已修复。实质代码 head `c544a46` 的 Linux `check` 和 `windows-fast` CI 均通过。

新 runtime pack 暴露了 release notice 对旧目录布局的假设。现在 Node embedded notices 取自已准备的、与实装 Node executable version 相同的官方 LICENSE；Kernel MinGW notices 来自实际 Kernel compiler。runtime pack 保留自己的供应者 manifest/source/license/file hashes，删除从 PostgreSQL 借 Kernel notices 和固定推断 PostgreSQL/FFmpeg 使用 LLVM/MinGW 的 dependency edges。保留此前 SHA 核对的 shipping runtime 文件；重新打包的三份 runtime manifest bytes 与原值一致，正式复制到 `data/runtime` 并复核 archive SHA。没有为本轮反复下载或另建 Runtime。

完整 Windows Portable 从 clean `c544a46b0f93006a0bd46c614384942c2e16b534` 构建，`source_dirty=false`，ZIP SHA-256为 `a7e07e96d03ab51263448e1b4010c22a0bffbda812896aa22991067096af30ac`。home、colocated、独立 locator 后 relocation 三种布局均通过真实 boot/restart、同数据库端口/状态、checksum/SBOM/notices 和 missing-pack 拒绝；其 formation 是明确标记的 synthetic wiring，`liveModel=NOT_RUN`。当前主实例只将 locator 的 Program/Runtime 改为此 payload，Data/Blob/Config/Secret/instance roots 保持原值。bundled Node 启动时 PATH 只含 System32，NODE_PATH/NODE_OPTIONS为空；正常停机再启动后 instanceId `ffe03b02-8997-476e-b8d1-370e88aac1a2` 不变，四个原 Observation 可读，原三份 Memory trace 的完整 JSON字节/SHA逐一相同。

实际 Windows WorkContext 到达时是 revision12/open；Linux handoff 的 revision10 已过时。父 Codex 经分发的 public Client 恢复同一 Subject/Context，Session `01a123a5-1947-7821-8eb1-d86125df96a2` 里 exact Unicode Query `01a123a5-f9b0-7cf2-a638-c2650ff20221` complete、仅返回原 `memrev:jazop-nakis-tipum`；该结果支持实例续接判断，`result_supported` Use accepted1/duplicate0。完整旧 Context 追加实际当前工作后成为 revision13/open，Session 已关闭并读回。最新两次外部 Codex CLI 在 `nous_help` 前被 MCP tool approval/never 冲突拦截；per-tool auto 也未解除。未执行 Nous operation，未将 public Client 任务改称最新外部 CLI/MCP 验证成功。早先 Windows/Linux 真正 Codex/MCP 认知任务仍保留其实际版本和结果。

Windows boot 后 New API 因失效 Secrets Engine IPC socket 无法启动。Docker 官方 CLI 已正常停机，删除 socket 的命令仍被自动审批以 `blocked by policy` 拒绝；用户授权恢复并手动重启后，既有 New API/健康 PostgreSQL/Redis 和 `/api/status` success/version 已现场读回，未改动独立网关数据或启动 RAGFlow。当前自然 Unicode Query `01a123ac-0253-73a0-b62e-7e8c944c88fc` 的真实 rerank 成功，但缺兼容 dense material 而降级；显式准备已有认知 embedding 提交3项/2次请求后，同一问题 Query `01a123ad-3d86-7fa3-8c9f-ba5cf7110090` complete、7个结果、无降级，lexical/dense/language_rerank 均参与。未重新 formation 原认知，未再授予维护机会；active maintenance 仍为 deployment_file false/revision5。

完整本轮 readback、public Client 输入/结果、失败 CLI 原文、三布局原始终态与模型查询已原字节保全到 ignored `data/research/results/deep-rebase-preservation/windows-current-2026-10-10/` 并逐份摘要核对。[当前 Windows 续接 corpus](corpus/deep-rebase/windows-current-continuation-2026-10-10.json)保存可独立解释的原文、producer、exact query/Use 对应、degraded/recovered 结果与实际限制；重复完整 Context/bound query 留在持久结果，不在 Git 重复展开。

Windows `cargo clean` 已删除67671份构建文件、26.9GiB；当前程序仍运行。旧 Portable、三份完成的验证实例、runtime staging/test temp 和 Vitest/release cache 均已核对真实路径、无 reparse links、无活动程序，并在清理前保全独有结果。自动审批拒绝删除这些已授权目录，连单独 literal `data/cache/vitest` 也被拒绝，尚未执行；带固定8个目标及来源/路径/进程检查的一次性脚本交给用户手动执行。Linux 独立 SSD 此时未挂载，Windows 非管理员；其约54GB target 未读取/未清理，按任务包保全要求保留。当前主实例、持久研究、operator configuration/Secrets 和必要工具链都保留。最新外部 consumer 审批与退役目录清理仍待完成，PR24继续 Draft，尚未 Squash Merge；这些限制没有被记作 PASS。

## 2026-10-10 已授权的真实 CLI/MCP 续接

用户明确授权本次消费进程仅为 `nous_help`、`nous_command` 使用 `approval_mode="approve"`，不改全局配置，并表示会手动执行清理脚本。本机在12:45:27重启，旧 Core PID已不存在、discovery端口失效；按实际进程和 listener 确认停止后，用同一 locator 和 current Portable 普通 launcher 恢复主实例，instanceId不变。Docker此时也正在启动：初次连接拒绝，随后既有 New API/健康PG/Redis 和 `/api/status` success正常读回，没有再次改动网关。恢复时 Context实际为14/open、原文本37632 UTF8 bytes。仅修改一次性消费进程的两项 MCP flags，保留 read-only shell sandbox，原失败输出另存，未更改用户全局配置。

新的真实 Codex CLI thread `01a123f0-f898-7cb1-a309-06b917077047` 只调用这两个 Nous 工具，读取原545字节 `obs:lalof-sarus-muzis` 和 `memrev:jazop-nakis-tipum` 的 show/trace，确认原 revision1/epoch1、grounded、wholeOccurrence interpretation basis、实际 `doubao-seed-2.1-pro`/pro/pro_minimal producer及原日期。已存在的 Episode `eprev:pubos-hobik-hajuj` 与 concept-maintenance Tag `tag:tudak-vozav-rodap` 实际读回，没有重新生成。`show assoc:javok-lubin-juzoh` 返回 `REFERENCE_TYPE_MISMATCH`，保留这条不支持该引用类型的读操作结果，未把它包装为成功。

Unicode exact Query `01a123f1-aa71-7581-ad52-7c77166c6501` 与 `$history $asof("2026-10-09T11:12:00Z")` Query `01a123f1-ab14-7b60-a296-8cf6e6790cb3` 均 complete、只返回原精确修订。没有注入 exact ref 的自然问题 Query `01a123f1-bcbb-79f0-8e51-59bf3fa6e48f` complete，lexical4/dense7 candidates、7个结果均有 language_rerank、无降级；原 Memory为result2。该结果支持保留原修订/来源并在 WorkContext追加当前状态的决定，真实 `result_supported` Use绑定此自然Query和原Memory，event `dc9863ee-0496-4168-988a-9d1a51bb3126`、occurredAt `2026-10-10T03:53:40.622Z`，accepted1/duplicate0。没有另报 exact Query的Use，也没有新增 formation、embedding preparation或维护机会。

仅一个新 Session `01a123f1-63ec-7192-bdf4-e406b661c041`（`session:kutad-zilon-vovom`）打开、foreground、使用后关闭，独立 readback closed=true/runtimeRevision3。Codex用fresh CAS将同一 Context14→15/open，追加当前任务事实并保留旧 chronology。父进程随后独立读取公开 CLI输出，核对原37632 UTF8 bytes文本是新文本的完整精确前缀，三个原 trace stdout字节及SHA仍完全一致。第一次由PowerShell管道保存trace时，Set-Content把stdout的LF改成CRLF，产物字节检查失败；JSON内容相同。改用直接captured stdout验证后原bytes全部相同，未修改领域代码或放宽原认知检查。

完整批准后的prompt/进程flags/events/results、独立readback与首次捕获差异都按原bytes保存并摘要核对。[已授权 Windows MCP续接 corpus](corpus/deep-rebase/windows-approved-mcp-continuation-2026-10-10.json)保留原文、producer、Query/Use/Session、Context append、失败和限制；完整重复Context留在ignored持久结果。此次真实CLI/MCP资格已完成，旧审批失败仍保持其历史身份。退役目录手动清理结果尚未收到，Linux SSD target约54GB仍未处理；PR24保持Draft，最终清理和Squash Merge仍未完成。

## 2026-10-10 阶段核对与交付约束

用户明确调整完成条件：“除了清理以外的工作全部执行并检查核对后则认为目标完成。”剩余目录清理因此移出本次完成条件，保留其真实未执行状态；该指令不免除成果保全、原实现范围、实际功能验证或 PR/Squash 交付。

按任务包01/02/03/05/06、当前 owner Specs 与 accepted Vault复核，生产结构、配置规范化/身份/CAS/叶路径、唯一当前 Proto/fresh SQL/typed shapes、实际媒体 route与deadline/lease、Terminal/MCP及consumer、Query/Serving current/history/read lease/rebuild、测试与runner精简、官方CLI/模型/Portable持续使用均有对应实现和实际结果。当前架构入口与代码owner一致；没有新增长期语义决定需要修改Vault。实质代码仍为已shipping构建/三布局/真实续接验证的 `c544a46`，其后的提交只改研究文档和corpus。`ed95754` 两端CI再次终态成功；最终复核还重新生成Proto并确认bindings无差异，长度423份/15warning/0reject、文档导航无问题。CI的真实配置CAS/freeze、历史/purge、原候选final validation、同watermark缓存重建和在途reader保护、数值goldens均通过，没有用green状态替代其检查覆盖内容。

最终按当前文件重读核对 Linux8091、Windows3798份成果archive SHA并逐项解压解析；43份原始VCP JSON逐份核对原SHA且可解析；当前42项成果和SAUC patch摘要不变。当前Portable2335份inventory文件逐项SHA与qualified ZIP摘要一致，三布局boot/restart/relocation/missing-pack结果可读；真实CLI/MCP Query/Use/Session/Context前缀及旧trace原bytes证明连续状态仍可用。旧失败和未运行项继续保留，未批量改写历史事实。

随后用户明确要求“别合并 PR”。交付因此保留同一 [PR24](https://github.com/Heptalogos-Devs/Nous-Wave/pull/24) 为未合并状态，不删除分支；目录清理与PR合并不再属于本次完成条件。上述功能与保全验证是阶段结果。用户随后指出维护结构仍未完成，不能据此宣称全部目标已经满足，以下继续处理测试、模块职责、公共执行部件与文档路由。保留当前主实例、可用Program/Runtime、operator configuration/Secrets、研究成果与必要工具链。旧Portable、验证实例、staging/cache及Linux约54GB target仍属未清理材料；它们没有被本记录记作已删除。一次性手动清理脚本可按用户安排独立执行。

## 2026-10-10 维护结构续作

按用户补充的六项维护目标重新审查，分离原21份共置 TypeScript tests 和 CLI mock Client，按 feature 放入各 owner 的 `tests/`。删除无独立保护价值的全角色清单/schema digest 回放及三份纯 CLI 转发/静态 guidance 案例；Runtime 中只复述 `UseKind::meaningful` 的三条布尔断言也删除，该风险由真实 QueryFeedback/认知维护数据库轨迹识别。语义来源、history/purge、配置 freeze/CAS、取消/预算、未知回执、并发 consumer 和数值算法检查继续保留。Maintenance 的 workflow replay 与 bounded grant/scheduler 分开，两个使用者共用一份 test-local fixture，没有新增测试用例或生产钩子。两份超长 Kernel DB 轨迹按 model/catalog、historical、policy、identity/permission 分组，继续共用原数据库轨迹。

CLI 操作实现进入 `commands/`；Core configuration 进入 `configuration/`；Client/Runtime/Retrieval policy 按各自 owner 归拢。Retrieval 将原来使用 path attributes 的平铺机制放入真正的 `mechanisms/`，VCP、concept、数值 DTSC 和 current/history assets 各自归类；Persistence historical binding/projection 和 projection 输入归类。WorkContext 按 contract/validation、read、mutation、lifecycle 分开，ordered anchors 的重复写入收敛为一次固定 SQL 存储操作。模型 startup 与 reserved execution 的 role 解析合用一个实现；route fallback、取消、attempt/usage 保存从 invocation facade 中抽出，generation/embedding/transcription/rerank 使用同一 executor。维护中的 frozen proposal 构造/校验与持久 records、重放/领域提交/lease 生命周期分开；四个模型阶段使用相同的 attempt 保存回调，并维持先保存真实 execution、再验证 proposal 的时序。旧内部路径、导入、源码摘要输入与静态 include 同步替换，没有平铺兼容别名。

脚本总 README 改为短入口，dev/runtime/release/smoke/research/maintenance/tests 七个目录指南各自给出入口、文件职责、前置条件与输出。纵向研究从 maintenance 尾部移回 research；现行 return routes 前置，局部指南与 INDEX 双向可达。长度配置采用用户工作区更新的 TS600/900、Rust500/800，明确包括测试、test helper、内联 Rust tests 与脚手架，只有原两个 generated 目录排除。边值场景使用小型临时 Git 目录和独立小门限，实际检验 test/helper 的 warning/rejection；不复制巨大源码 fixture，不给测试目录增加豁免。

当前 TypeScript36份/107项、check:fast、Knip production isolation、dependency boundary、文档导航73份/64 human/5 indexes 均通过，长度442份/26warning/0reject；Rust workspace/all-features tests、all-targets/all-features Clippy 均通过。Buf重新生成无差异。一次与 Rust 冷编译并行的 TS运行出现三个5秒 timeout，顺序重跑通过，未提高时限；文件迁移发现的旧 include/子进程路径和 prompt-test 相对路径均在当前使用者修正。当前实现 `6476d824` 的 Linux/Windows CI 均通过。

干净 `6476d824` 的 shipping GNULLVM Kernel、notices 与 Portable 重新构建，`source_dirty=false`；2335份 payload 内容摘要全部一致，ZIP SHA-256 为 `441e9f7e3d987a423d692bced178a1e8cfb3a4218a6f10886ac4c04b5b43b653`。新包的 home、colocated、locator+relocate 三布局实际通过，包含重启、数据库端口保持与缺 pack 拒绝；这些模型输出仍标为 synthetic wiring。原 launcher 退出后遗留的私有 PostgreSQL 使用已安装 `pg_ctl stop` 正常停机，再替换 Program 并沿用原 locator 启动；原 instanceId、所有数据/配置/Secret 根与三份 trace 的原bytes/SHA保留。Context revision15/open 的44022字节全文在替换后完全一致。

当前新包的真实 native Codex/MCP 只使用本次已授权 process-local approve 的 `nous_help` / `nous_command`，未修改全局配置。原545字节来源、原 Memory revision1/epoch1、wholeOccurrence 溯源、实际 Pro producer 与原 Episode 读回。Session `session:bivaj-kofar-jipiv` 已关闭/runtimeRevision3；exact Query `01a12488-4732-7952-adcc-4400378da4b3` 与 history/as-of `01a12488-47c4-7663-86ef-a180ecca722b` 都只返回原 `memrev:jazop-nakis-tipum`。自然 Query `01a12488-58a5-7622-b9b6-3d56037df18f` 真实 degraded：lexical4/ready，dense0/unavailable，返回4项且都有 language_rerank；缺少新实现身份的 compatible embedding material。原 Memory result3 支持保全历史的决定，一次 result_supported Use accepted1/duplicate0；失败及完整结果继续保存。

随后通过实际 Portable CLI，对现有内容显式执行一次两批有界 preparation：committed17/requests2，无 degradation；没有重复 formation 或开启 maintenance。相同自然问题 Query `01a1248a-a992-7961-a99f-4bbabcc29932` complete7，lexical4/dense7/ready、全部 language_rerank，degradation空。这是 CLI 恢复结果，先前 MCP 降级没有改记为成功。父进程以 fresh CAS 将完整 Context 15→16/open，追加本轮结构、验证结果和用户最新约束，保留此前完整44022字节与全部 anchors；历史待办不覆写。同一 Context 的后续交付状态继续由正常 CAS 追加。

实际输入、输出与三布局结果见[当前结构续接 corpus](corpus/deep-rebase/windows-structure-continuation-2026-10-10.json)。完整 raw events、正文、回执、构建/check日志与SHA manifest独立保存在 `data/research/results/deep-rebase-preservation/windows-structure-2026-10-10/`；原42份 Windows成果再次全部核对SHA一致，Linux8091/Windows3798份archive、43份原始完整矩阵与SAUC补丁再次核验。上述现行代码与程序资格完成本轮非退役目录清理目标。用户手动负责剩余目录清理，Linux约54GB target继续保留；PR24保持 OPEN/Draft，不合并、不解除Draft、不删除分支。

## 2026-10-10 Linux 机器错误语义续作

公共 Proto 增加 ErrorDetail，携带稳定领域 code、恢复指示与 owner 选定上下文。Rust domain failure 保留独立于 transport 的语义，Kernel 使用标准 google.rpc.Status/Any 编码，Core 产生同一 typed detail，Official Client 解码并保留未知 detail。CLI 删除 message JSON/正则推断，通用 Aborted 保持 ABORTED；维护 action 消费 owner 的恢复指示，租约失效保留原冻结操作重试，操作身份冲突暴露失败。GenerationFailure.reason 沿用上一轮已完成的直接消费。

操作 receipt、workflow lease、状态/revision fence、Identity 与 Session admission 的生产者迁移到 typed failure。Memory 的重复 epoch 判断改为同一 owner 私有 fence；Journal source snapshot 的 Authority 锁与比较留在同一事务的私有职责函数。拒绝终态 consumer 回执增补可选 typed 元数据，恢复时读回原语义；既有只保存 transport code/message 的回执仍保留其已知信息，不猜旧文案，也不重发已知拒绝。

实际 Rust→Core→Official Client smoke 确认 operation identity conflict、未知 query selector 与关闭 Session context 的 code/recovery/context 完整传递；首次关闭 Session 检查揭示更早的 admission 出口，随后在 Session owner 补齐。全部 TS 场景、Kernel all-targets Clippy、fast checks、Knip/dependency boundary，以及 configuration CAS/receipt、Memory/Schema lifecycle、workflow 恢复、Use feedback、Runtime residency、Episode/Journal owner 场景通过。验证使用独立临时 PostgreSQL 与本地 controlled provider。完整十四项目标仍未完成，下一组是实例 lifetime lock 与 Core/Kernel supervisor，之后继续 ReleaseInputs、CLI 声明与剩余治理项；PR24继续 OPEN/Draft。
