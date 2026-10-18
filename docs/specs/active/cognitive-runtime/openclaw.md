# Managed OpenClaw

[返回合同目录](../../INDEX.md)

## 所有权

Nous 提供唯一的长期认知。官方 Client 的 `./managed` 子入口提供框架无关 prepare/apply/admit/report/close；`packages/openclaw` 只映射原生循环、受众和实际结果。Heptalogos 可以装载同一发行物，其重建、数据库及旧 AIRuntime 不是 Nous 集成的前提。

Subject 是独立持久个体，创建可省略 Seed；没有 Seed 不生成个人经历、默认人格或可写 SOUL 镜像。初始源文档的保存/adoption 与后续 Self 形成分别处理。Self 的领域功能见 [Self](../self/self.md)；Social/LanguageConvention/Motivation 按长期设计继续实现。

## 实际调用

每次原生模型提交在 convertToLlm 后、provider stream 前运行 managed binding。首轮、工具后续、重试、压缩后的下一轮和原生子 Agent 均经过该入口；terminal completion 仅保存结果，显式释放后产生的新主体调用仍经过同一入口。必需模式不允许 context-engine registry、logical-turn、assemble 或其他 harness 失败后退回 legacy。安装器将修正入口应用到哈希固定的 OpenClaw 2026.9.1；漂移/部分补丁拒绝安装。

`CognitionUsage` 从实际执行者、用途、当前意图、参与者与受众形成。它进入已有 Query binding 和 ContextCompiler track；不授予管理权限。adapter 从原生可信 chatType/channel/sender/destination 获取来源，未解析的人物保留 unresolved；不自行分配 Entity。身份、principal、Subject 与受众不从模型工具参数取得。

受控边界额外要求`providerCustodyVersion: 1`。原生SessionManager唯一持有实际provider请求的append-only custom记录；它们不进入模型history。现有OpenAI chat/responses通过SSE HTTP执行，在provider SDK完成全部onPayload修改及具体重试改写后、create前等待完整body保存成功。请求保存失败时阻止提交；HTTP状态、request ID、实际usage和终态以同attempt身份追加。transport headers/API key不保存。尚无确认结果的请求为uncertain；响应usage缺失时保持未知。

Host 保有历史、工具 schema、输出预留和 provider controls。renderer 只在干净的调用输入上应用一个 Nous slice，固定指引说明认知、证据和归属的使用方式；authority 不转换为管理/工具权限。slice 不持久写入 transcript。原生 system prompt 不再加载 SOUL/IDENTITY/USER/MEMORY/BOOTSTRAP 为第二份当前认知。

## Host Life 与实际接触

可选 Host bridge 使用官方 Client `/host-bridge` 的单一公开 wire 合同。操作者配置的 loopback origin 与私密 credentialFile 决定 Host，不从模型参数获得。v5 严格 provider 边界在每次调用前等待 Host 核验已持久 NativeRun/Activity、原生身份、prompt digest 与受限来源；缺少许可则终止。Host canonical usage 与结构化 contacts 分别进入 CognitionUsage/ObservationInput 的正式 fromJson，不另造认知对象。HostPrepareResponse区分当前工作与已保存义务：正常调用提供当前workContextId；Narrative owner确认原candidate仍有pending索引义务后，可返回continuationSourceRef而省略当前WC，仅限execution-only narrative.review且无工具。插件用正式SetActiveWorkContext CAS清空Session旧绑定，prepare/query不传原任务WC作为当前工作；registration及SourceRead保留原任务lineage。原WC的closed状态、来源撤回和Core历史检查继续适用。Hosted user消息不从工程prompt重复生成泛化Observation。

Host `currentSources` 使用既有 `EvidenceRef` 选择 actual occurrence 的 whole/source-region/derived-representation/derived-region locator，交正式 `currentMaterials`；选择不增加 `SourceRead` 权限。Material owner 在当前 scope 下核对 occurrence 与 locator 的真实 lineage，SDK 只采用 exact locator 与 actual occurrence 均匹配的读回结果，不复制旧投影正文或重派生既有材料。

工具/模型结果先由原生 transcript 持久非秘密 contact identity 与真实事件，再执行 Host pending → Nous receipt → Host committed。Host 保存 pending 与补交，插件不拥有第二个 durable queue。下一新 turn 和 compact 前读取正式完整 transcript 严格补交旧 contact；保留原 Activity、作者、来源和现实接触时间，不能用新 Activity 重新归属旧结果。原生 reset 保留原 transcript boundary；回写失败不是重复执行或发送的依据。子 Agent 在正式 isolated handoff 中接受同一 Host 的受限继承，不获得维护或渠道发送权限。公开路径、字段、配置、自动验证和实际集成边界统一见[包说明](../../../../packages/openclaw/README.md#host-life-与结构化接触)。

v5 的 Host 工具列表先过滤模型可见工具，真实 internal preparer 与最终执行前再强制等待同一 Host 的 tool-execution 检查；不能使用 best-effort hook 替代。Hosted `nous_query`/`nous_read` 的来源许可来自实际执行 guard 的私有上下文，不消费模型参数或静态跨群 MCP scope。工具正文进入历史前先正式记录实际返回 refs 的 presented 使用；原来源失效继续封闭后续模型输入。工具资源的 OS 隔离由 Host 负责，工具名集合不声称隔离文件系统。

## 同步与预算

prepare 复用 BuildManagedContext；apply 成功后才推进本 binding 的 cursor。RESET 替换全部 slice；APPEND 仅接受相同 track/epoch 和连续 revision。受众、用途、执行者、WorkContext 或内容变化会改变 scope/RESET。并行 worker 分开保存临时状态。`ManagedBinding.appliedProjection(applied)` 从本 binding 的真实 apply 返回完整官方 Projection；APPEND 的既有前缀与 EpistemicView 不会在 Host admitted 回执中丢失，Host 可据此构造表达 brief，不从模型自报认识重建认知。

Host 的受信 tokenBudget 指定完整 inputTokens 与 outputTokens，允许工具由 RoleBinding 与当前 Activity scope 取交集后发给原生可见面和实际执行 guard。缺预算/工具政策的旧 RoleRun 标为 unconfigured：保留已有成果、终态和真实送达回流，禁止新增 provider attempt，不推断默认角色预算。新子 Agent 从真实 execution profile 取得显式政策，不把父审议预算冒充子角色配置。

Host 可随许可返回受信 responseFormat：结构角色使用所属 owner 的 strict json_schema，独立自然文本角色显式使用 text。SDK 将该值交给受控原生 stream owner，在每次实际 provider 调用的最终选项中应用；同一格式随真实接触 marker 保存并用于结果反馈与重启补交。

Host 按所选模型的上下文窗口、输出预留、原生完整输入估计剩余预算，再取 role input cap 与 cognition cap 的较小值。最终应用后的 system、tools、history、media 和 Nous slice 一并复核；超限停止这次调用。providerControls.maxOutputTokens 在受控 native stream owner 最终传给 provider，取角色 cap 与实际原生 profile 输出限制的较小值，不覆盖更小的已配置上限。当前 token 计量是 OpenClaw native estimate；Nous 文本字节上限用于有余量的输入控制，不声称精确 tokenizer。renderer 核验最终输入，超预算停止调用。

WorkContext 和 identity/situational guidance 必须完整；不足时拒绝。Memory/Schema/Episode/Journal/Material 等可选语义单元整段省略并报告，不截断条件。REQUIRED 意味着认知 owner/capability 必须可用，不意味着无相关认知的新主体必须凭空有一段记忆。Kernel 核验必需 Memory 的进程和 Subject 能力；来源读取或传输失败照实际错误处理。

## 原生循环与结果

OpenClaw 继续拥有工具执行、渠道、Computer use、skills、子 Agent 和 transcript。压缩显式委派 `delegateCompactionToRuntime`；其输入不含 Nous slice，下一轮重新应用。默认 memory flush 对主体实例关闭，避免第二套可写认知。

原生 session extension 仅保存 Subject/nativeSession/NousSession 身份，不保存 cognition 或 cursor。重启复用未关闭的对应 Session；native reset 使用新 Nous Session；实际 reset entry 标识其历史代际，不假定物理 native sessionId 必然改变。原生会话扩展按 pluginId/namespace 保存；运行时注册表卸载与显式插件禁用的持久状态清理分别处理。子 Agent 同 Subject、独立 Session/binding，在正式委派准备入口保存父 Session 关系，再派发子任务，以隔离任务交接，不能经模型要求 fork 父历史。

授权详情通过`nous.provider-attempts`只读RPC和正式Client host-bridge schema取得；RPC要求operator.admin，按实际Subject/native run/agent/session核对，默认只给metadata，includeInputs显式读取完整provider body。旧历史无body时返回not_recorded并保留可查的原assistant用量，不重新调用模型补造请求。原生cost字段是配置价表估值，实际provider账单金额未知。

正式 Host bridge 每次许可返回受信 role/purpose/view/contactNature。角色覆盖 world.planner、subject.deliberation、subject.execution、subject.render、narrative.review、subject.reflection 与 system.maintenance；技术 Agent ID 不等于产品角色。Host 从已采用行为和真实 native binding 取得用途，模型参数不能选择视角或提升权限。subject deliberation/render 使用必需主体投影；world author、worker 与 review 使用各自受限资料，隐藏世界内容在候选检索前排除。切换用途需要独立干净 Session/binding，不把旧 worker transcript 带入主体历史。

execution_only 的实际模型/工具输出与失败结果只留在所属 RoleRun/native transcript，Host pending 返回 retainedOnly 后官方插件不调用 RecordObservation，不进入主体 Episode、反思或统一检索。主体实际接触由显式有界 release 创建新的 subject_experience contact，引用原运行和被释放的精确结果；释放范围以外的内部推理和工具资料不导入。standalone Nous/OpenClaw 仍保留明确的主体体验路径，真实 user/model/tool 接触可形成 Observation；无 Host 时不发明世界权限或来源。模型输出始终标为 generated/model_output，不证明实际外发。

同一 native history generation 不得跨产品 role/view 复用；Host 拒绝已经装载另一 roleScope 的历史，要求新 Session 或正式 reset。原生 converter 为 compaction/branch summary 添加受信元数据；普通无许可 user/steering 不能冒充压缩摘要，未获许可的调用及其工具/模型因果链不进入 provider。

子 Agent terminal completion 只核对不可变父绑定、保存 exact terminal 结果并提供 activity_result 机会；恢复同一付费结果不重跑 worker。不得把 completion 自动注册为 main/final-send continuation，也不得将 worker 全部结果直接提升个人知识。正式释放后进入主体审议、独立 subject.render、narrative.review、NarrativeCommit，再交唯一发送 owner。renderer 输出只是候选，无法自行发送。

发送前的 admit 使用现有 ReportUse 与 `model_input=true`：来源仍可用、Session 及父链历史安全，才返回 `model_input_checked=true`。没有认知来源的输入仍做 Session 检查；旧 Core 没有确认时停止发送。该操作登记真实待发送输入中的 exact refs，不为每个片段创建另一层持久 receipt。subject_experience 结果在 report 中以实际 outcome Observation 记录；execution_only 结果交所属 RoleRun，不写观察。失败和取消不冒充执行成功。

Runtime Session 保存 parent_session_id、history_invalidated 与已进入模型历史的认知域。purge 在删除 UseEvent 前将受影响 Session 及子会话历史失效；suppression/withdrawal 使用同一边界。发送前同时检查父链，覆盖子会话创建与清除并发时的可见性。普通正文修订仍使用当前认知 RESET，保留旧历史，不作为清除操作。

历史失效后不允许继续从旧 transcript 或 compact summary 发送。插件通过持久 native Session extension 保留对应关系，Core/Gateway 重启不能重新绑定成干净 Session。正式 native reset 清除原生当前历史、建立新 Native/Nous Session，继续消费同一 Subject 与 WorkContext。consumer 收窄为 FORBIDDEN 时，曾使用该域的旧历史也拒绝进入模型。独立且未接触该来源的 Session 继续可用。

ReportUse 仅把实际待发送输入的 exact cognition refs 记为 presented。模型产生引用或文本不证明其内部因果、选用或真实外发。Heptalogos 对实际行为/外发许可及 effect/messaging outcome 负责，不能以 best-effort message hook 替代正式许可入口。受控渠道通过 `ChannelPlugin.message.deliveryCustody` 在原生 prepared batch/queue 中持有 Host 签发的非秘密 adoption reference；新接纳先验证可信 native run/session，恢复复用既有 reference，每个实际 adapter handoff 必须等待当前 Host 授权。缺 provenance/custody 或 queue 持久化时拒绝发送，通用渠道不声明此接口则保持原生能力。具体 SDK 合同和无网络 fixture 命令见包说明。

普通聊天“记住/忘记”不拥有认知 mutation/purge 能力。工具可见性、执行政策、凭据和 OS 范围由宿主明确配置；插件不向普通主体暴露 Core discovery 管理凭据。运维 OpenClaw 使用独立实例及权限。

## Reactor、继续信息与固定前缀压缩

本节是当前Reactor整包采用的新增实施合同；已有正式role/view、结果保管、预算与原生compaction委派继续使用，后台固定前缀生成/发布完成实际入口和真实长任务验证前不宣称可用。Heptalogos拥有同Subject一条前台决定链、焦点、情境动作及同步继续信息；本集成不另建Reactor或每域认知装配。

主观审议/renderer消费同一适用EpistemicView；renderer为独立自然文本调用，不要求FC，执行模型的末段不是渲染。世界、worker、review和compactor的原始资料execution_only；终态只持久结果和机会，主体具体查看时显式release，新用途不fork父完整历史。WorldProfile/可选Seed分别采用，合法world可见来源/域时间与知识历史沿同一正式类型贯通，不造平行DTO。

关键结果先由owner持久，目标变化、委派让出、切换、完成、取消及正常停止前同步保存最小Activity/WorkContext继续信息，引用真实结果，不等摘要、不加日记模型、不重复付费工具。摘要只维持运行历史，不自动形成Memory/Self或新证人。

OpenClaw独占transcript与摘要发布。soft在完整回合自然节点选择固定前缀，保留完整tool pair及未结束pair之外的切点，捕获原生Session/history generation、role/view、当前摘要代际、结束entry与来源许可；事务外生成。发布短CAS核验同代/未改写/未被后摘要覆盖/来源仍准后，只替换捕获前缀，保留未覆盖历史与新追加尾部。每Session最多一作业，manual/automatic/overflow合流；不建第二摘要队列或数据库。

H=min(实际窗口-输出预留-计量余量,role输入cap)，system/工具/媒体/历史/Nous slice共同计入；soft工程初值70%H、目标约50%H，不覆盖已有明确原生设置，不丢必需身份/目标/限定。soft失败且当前实际输入可容纳则继续并保留问题；硬超窗等待同一有效作业或同owner一次必要前台压缩，仍不足则保留历史并拒provider。换模型重算H，不因摘要模型较大扩大实际窗口，不重置预算或增本层fallback列表。

当前Nous slice、WorldView、焦点、权限与时钟排除于永久摘要并在调用时重建。普通修订由RESET刷新；purge/禁传影响的transcript及派生摘要先history invalidation，再正式reset/清理，后台旧产物不得重新发布或跨群复活。摘要继承原role许可、独立记录用途/成本而非全知人格；原生memory flush对Subject继续关闭。

真实验收需在同安装长任务后台压缩时追加新输入和真实tool结果，发布后继续原工作，验证尾部/完整pair/未决事项；覆盖较小模型硬等待、焦点切换、取消、来源限制与重启。仅mock、摘要函数返回或现有同步compact通过不能替代本新增边界。

## 发行与验证

[包说明](../../../../packages/openclaw/README.md) 是安装/配置命令的唯一完整入口。`openclaw:pack` 生成 bundled 插件及安装器 tarball，宿主原样安装；`openclaw:prepare` 使用同一产物安装到开发实例。 原生安装使用发行物的 `nous-openclaw-install`：安装根持有固定传递依赖的 npm overrides、lockfile 和可重装插件 tarball；依赖包自身的 overrides 不被当作消费者保障。准备器在原始/补丁哈希前核验 OpenClaw 与 proxyline 解析到的 Undici、MCP SDK 实际版本；不接受旧易受影响的解析路径。

`pnpm openclaw:custody:verify` 使用正式 prepare 与真实原生执行代码，检查 SQLite queue、发送授权、历史压缩、工具绑定和子任务交接；真实 provider 输入与 QQ 交付沿产品原任务验证。

Self 的付费行为场景使用同一正式安装入口，检查无 Seed 自主选择、有 Seed 形成、实际公共资料工具步骤、同事项反思修订及旧 exact revision、话题/模型/会话/进程连续性和普通聊天的管理权限边界。命令和模型配置要求见包说明。模型提案仍需按实际语义与来源核对，不能仅凭合法结构或出现名字判定通过。

受控 runtime 可以由同一 installer 显式安装声明所需 native state 的本地插件。仅 `openChannelIngressQueue` 与 `openKeyedStore` 按实际 artifact、npm lock identity、安装文件完整性和准确插件入口授予；不是官方 npm provenance，也不提升其他可信能力。安装锁与真实状态持有进程 lease 要求离线替换/移除，记录删除只影响后续 grant，不代替终止已运行对象。其他用户插件、npm metadata 与 profile 保留；完整命令和字段由包说明维护。
