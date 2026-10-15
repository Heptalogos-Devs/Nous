# Managed OpenClaw

[返回合同目录](../../INDEX.md)

## 所有权

Nous 提供唯一的长期认知。官方 Client 的 `./managed` 子入口提供框架无关 prepare/apply/admit/report/close；`packages/openclaw` 只映射原生循环、受众和实际结果。Heptalogos 可以装载同一发行物，其重建、数据库及旧 AIRuntime 不是 Nous 集成的前提。

Subject 是独立持久个体，创建可省略 Seed；没有 Seed 不生成个人经历、默认人格或可写 SOUL 镜像。初始源文档的保存/adoption 与后续 Self 形成分别处理。Self 的领域功能见 [Self](../self/self.md)；Social/LanguageConvention/Motivation 按长期设计继续实现。

## 实际调用

每次原生模型提交在 convertToLlm 后、provider stream 前运行 managed binding。首轮、工具后续、重试、压缩后的下一轮、原生子 Agent 和完成回告均经过该入口。必需模式不允许 context-engine registry、logical-turn、assemble 或其他 harness 失败后退回 legacy。安装器将修正入口应用到哈希固定的 OpenClaw 2026.9.1；漂移/部分补丁拒绝安装。

`CognitionUsage` 从实际执行者、用途、当前意图、参与者与受众形成。它进入已有 Query binding 和 ContextCompiler track；不授予管理权限。adapter 从原生可信 chatType/channel/sender/destination 获取来源，未解析的人物保留 unresolved；不自行分配 Entity。身份、principal、Subject 与受众不从模型工具参数取得。

Host 保有历史、工具 schema、输出预留和 provider controls。renderer 只在干净的调用输入上应用一个 Nous slice，固定指引说明认知、证据和归属的使用方式；authority 不转换为管理/工具权限。slice 不持久写入 transcript。原生 system prompt 不再加载 SOUL/IDENTITY/USER/MEMORY/BOOTSTRAP 为第二份当前认知。

## 同步与预算

prepare 复用 BuildManagedContext；apply 成功后才推进本 binding 的 cursor。RESET 替换全部 slice；APPEND 仅接受相同 track/epoch 和连续 revision。受众、用途、执行者、WorkContext 或内容变化会改变 scope/RESET。并行 worker 分开保存临时状态。

Host 按所选模型的上下文窗口、输出预留、原生完整输入估计剩余预算，再取 cognition cap 的较小值。当前 token 计量是 OpenClaw native estimate；Nous 文本字节上限用于有余量的输入控制，不声称精确 tokenizer。renderer 核验最终输入，超预算停止调用。

WorkContext 和 identity/situational guidance 必须完整；不足时拒绝。Memory/Schema/Episode/Journal/Material 等可选语义单元整段省略并报告，不截断条件。REQUIRED 意味着认知 owner/capability 必须可用，不意味着无相关认知的新主体必须凭空有一段记忆。Kernel 核验必需 Memory 的进程和 Subject 能力；来源读取或传输失败照实际错误处理。

## 原生循环与结果

OpenClaw 继续拥有工具执行、渠道、Computer use、skills、子 Agent 和 transcript。压缩显式委派 `delegateCompactionToRuntime`；其输入不含 Nous slice，下一轮重新应用。默认 memory flush 对主体实例关闭，避免第二套可写认知。

原生 session extension 仅保存 Subject/nativeSession/NousSession 身份，不保存 cognition 或 cursor。重启复用未关闭的对应 Session；native reset 使用新 Nous Session；实际 reset entry 标识其历史代际，不假定物理 native sessionId 必然改变。原生会话扩展按 pluginId/namespace 保存；运行时注册表卸载与显式插件禁用的持久状态清理分别处理。子 Agent 同 Subject、独立 Session/binding，在正式委派准备入口保存父 Session 关系，再派发子任务，以隔离任务交接，不能经模型要求 fork 父历史。

记录实际 user/model/tool 事件为 Observation；工具结果包含结构、details 与 isError，并在原生结果发出后立即报告，覆盖终止工具。工具接触可 admit 成为经历；普通 transcript 不自动成为 accepted Memory。模型输出标为 generated/model_output，不声称渠道已发送。流式失败/取消报告实际 invocation outcome。

发送前的 admit 使用现有 ReportUse 与 `model_input=true`：来源仍可用、Session 及父链历史安全，才返回 `model_input_checked=true`。没有认知来源的输入仍做 Session 检查；旧 Core 没有确认时停止发送。该操作登记真实待发送输入中的 exact refs，不为每个片段创建另一层持久 receipt。结果在 report 中以实际 outcome Observation 记录；失败和取消不冒充执行成功。

Runtime Session 保存 parent_session_id、history_invalidated 与已进入模型历史的认知域。purge 在删除 UseEvent 前将受影响 Session 及子会话历史失效；suppression/withdrawal 使用同一边界。发送前同时检查父链，覆盖子会话创建与清除并发时的可见性。普通正文修订仍使用当前认知 RESET，保留旧历史，不作为清除操作。

历史失效后不允许继续从旧 transcript 或 compact summary 发送。插件通过持久 native Session extension 保留对应关系，Core/Gateway 重启不能重新绑定成干净 Session。正式 native reset 清除原生当前历史、建立新 Native/Nous Session，继续消费同一 Subject 与 WorkContext。consumer 收窄为 FORBIDDEN 时，曾使用该域的旧历史也拒绝进入模型。独立且未接触该来源的 Session 继续可用。

ReportUse 仅把实际待发送输入的 exact cognition refs 记为 presented。模型产生引用或文本不证明其内部因果、选用或真实外发。Heptalogos 后续对实际行为/外发许可及 effect/messaging outcome 负责，不能以 best-effort message hook 替代正式许可入口。

普通聊天“记住/忘记”不拥有认知 mutation/purge 能力。工具可见性、执行政策、凭据和 OS 范围由宿主明确配置；插件不向普通主体暴露 Core discovery 管理凭据。运维 OpenClaw 使用独立实例及权限。

## 发行与验证

[包说明](../../../../packages/openclaw/README.md) 是安装/配置命令的唯一完整入口。`openclaw:pack` 生成 bundled 插件及安装器 tarball，宿主原样安装；`openclaw:prepare` 使用同一产物安装到开发实例。 原生安装使用发行物的 `nous-openclaw-install`：安装根持有固定传递依赖的 npm overrides、lockfile 和可重装插件 tarball；依赖包自身的 overrides 不被当作消费者保障。准备器在原始/补丁哈希前核验 OpenClaw 与 proxyline 解析到的 Undici、MCP SDK 实际版本；不接受旧易受影响的解析路径。

`smoke:openclaw` 使用真实模型 gateway 与正式安装的原生 Gateway，核验最终 provider 输入、工具后续、RESET、原生 compact、子 Agent、真实 Material 回流、必需 Core 不可用时零发送及恢复，并核验真实自我选择的输出进入实际 native compaction summary 后，purge 和进程重启均阻止发送，正式 reset 后新输入不含旧标记。准备/应用日志不冒充已传输证明。未测试的渠道、Computer use 平台和语言人格质量不由该接线场景推定。

Self 的付费行为场景使用同一正式安装入口，检查无 Seed 自主选择、有 Seed 形成、实际公共资料工具步骤、同事项反思修订及旧 exact revision、话题/模型/会话/进程连续性和普通聊天的管理权限边界。命令和模型配置要求见包说明。模型提案仍需按实际语义与来源核对，不能仅凭合法结构或出现名字判定通过。
